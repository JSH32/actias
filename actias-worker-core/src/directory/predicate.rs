//! The predicate tree and its translation to parameterized sql.
//! Two hard properties, both tested:
//! identifiers come only from the shape's closed slot set, and values
//! travel only as bound parameters. A field the shape never declared
//! refuses by name; a building field refuses rather than serving a
//! partial index.

use std::collections::HashSet;

use super::shape::{FieldKind, Shape, Value};

/// The most values one `in` list may carry.
pub const IN_LIST_CAP: usize = 1024;

/// One comparison operator of the grammar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compare {
    Eq,
    Ne,
    Lt,
    Lte,
    Gt,
    Gte,
}

impl Compare {
    fn sql(self) -> &'static str {
        match self {
            Compare::Eq => "=",
            Compare::Ne => "<>",
            Compare::Lt => "<",
            Compare::Lte => "<=",
            Compare::Gt => ">",
            Compare::Gte => ">=",
        }
    }
}

/// One condition; a [`Where`] conjoins several.
#[derive(Debug, Clone)]
pub enum Condition {
    Compare {
        field: String,
        op: Compare,
        value: Value,
    },
    In {
        field: String,
        values: Vec<Value>,
    },
    StartsWith {
        field: String,
        prefix: String,
    },
    /// Membership in an array field.
    Contains {
        field: String,
        value: Value,
    },
    /// Presence (or absence, `present: false`) of the field. The only
    /// way to query for absence: comparisons never match an absent
    /// field, by design.
    Exists {
        field: String,
        present: bool,
    },
    /// OR over sub-wheres.
    Any(Vec<Where>),
    /// Explicit AND over sub-wheres, for grouping inside an `Any`.
    All(Vec<Where>),
    /// NOT over sub-wheres.
    None(Vec<Where>),
}

/// A conjunction of conditions: one `where` table of the Lua surface.
#[derive(Debug, Clone, Default)]
pub struct Where(pub Vec<Condition>);

/// One `order` entry.
#[derive(Debug, Clone)]
pub struct Order {
    pub field: String,
    pub descending: bool,
}

/// Resolves a field to its overlay column and predicate family,
/// refusing unknown and building fields. The unknown refusal is the
/// storage layer's own typo guard; the analyzer catches the same
/// mistake earlier when it can.
fn resolve(
    field: &str,
    shape: &Shape,
    building: &HashSet<String>,
) -> Result<(String, FieldKind), String> {
    if building.contains(field) {
        return Err(format!(
            "directory field '{field}' is still building; queries on it are refused until the backfill completes"
        ));
    }
    match (shape.slot(field), shape.kind(field)) {
        (Some(column), Some(kind)) => Ok((column, kind)),
        _ => Err(format!("'{field}' is not a directory field of this class")),
    }
}

/// Resolves a field and requires the scalar predicate family; array
/// fields point the caller at their own operators.
fn resolve_single(
    field: &str,
    shape: &Shape,
    building: &HashSet<String>,
) -> Result<String, String> {
    let (column, kind) = resolve(field, shape, building)?;
    if kind != FieldKind::Single {
        return Err(format!(
            "'{field}' is an array field; arrays take contains and exists, not comparisons"
        ));
    }
    Ok(column)
}

/// Resolves a field for a cursor clause, which needs the same
/// refusals as any predicate but builds its own comparison.
///
/// # Errors
/// Same refusals as [`where_clause`].
pub fn resolve_for_cursor(
    field: &str,
    shape: &Shape,
    building: &HashSet<String>,
) -> Result<String, String> {
    resolve_single(field, shape, building)
}

/// Renders one condition into `sql`, pushing its values onto `params`.
fn render(
    condition: &Condition,
    shape: &Shape,
    building: &HashSet<String>,
    sql: &mut String,
    params: &mut Vec<Value>,
) -> Result<(), String> {
    match condition {
        Condition::Compare { field, op, value } => {
            if !value.is_scalar() {
                return Err(format!(
                    "'{field}' compares against an array; the operand is a scalar"
                ));
            }
            let column = resolve_single(field, shape, building)?;
            sql.push_str(&column);
            sql.push(' ');
            sql.push_str(op.sql());
            sql.push_str(" ?");
            params.push(value.clone());
        }
        Condition::In { field, values } => {
            if values.len() > IN_LIST_CAP {
                return Err(format!(
                    "'{field}' in-list carries {} values; the cap is {IN_LIST_CAP}",
                    values.len()
                ));
            }
            let column = resolve_single(field, shape, building)?;
            // An empty list matches nothing, honestly and without
            // bending sql syntax around a zero-parameter IN.
            if values.is_empty() {
                sql.push_str("1 = 0");
                return Ok(());
            }
            sql.push_str(&column);
            sql.push_str(" IN (");
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    sql.push_str(", ");
                }
                sql.push('?');
                params.push(value.clone());
            }
            sql.push(')');
        }
        Condition::StartsWith { field, prefix } => {
            let column = resolve_single(field, shape, building)?;
            // Range form instead of LIKE: no escaping questions, and
            // it walks an index on the column.
            sql.push_str(&format!("({column} >= ? AND {column} < ? || x'ffff')"));
            params.push(Value::Text(prefix.clone()));
            params.push(Value::Text(prefix.clone()));
        }
        Condition::Contains { field, value } => {
            if !value.is_scalar() {
                return Err(format!("'{field}' contains takes a scalar member"));
            }
            let (column, kind) = resolve(field, shape, building)?;
            if kind != FieldKind::Many {
                return Err(format!(
                    "'{field}' is a scalar field; contains queries arrays, use a comparison"
                ));
            }
            // json_each over the stored json array; the member binds
            // as a parameter like every other value.
            sql.push_str(&format!(
                "EXISTS (SELECT 1 FROM json_each({column}) WHERE json_each.value = ?)"
            ));
            params.push(value.clone());
        }
        Condition::Exists { field, present } => {
            let (column, _) = resolve(field, shape, building)?;
            sql.push_str(&column);
            sql.push_str(if *present { " IS NOT NULL" } else { " IS NULL" });
        }
        Condition::Any(branches) => {
            render_group(branches, "OR", false, shape, building, sql, params)?
        }
        Condition::All(branches) => {
            render_group(branches, "AND", false, shape, building, sql, params)?
        }
        Condition::None(branches) => {
            render_group(branches, "OR", true, shape, building, sql, params)?
        }
    }
    Ok(())
}

/// Renders a combinator's branches joined by `joiner`, negated for
/// `None`. An empty combinator is a declaration mistake, refused
/// rather than guessed at.
fn render_group(
    branches: &[Where],
    joiner: &str,
    negate: bool,
    shape: &Shape,
    building: &HashSet<String>,
    sql: &mut String,
    params: &mut Vec<Value>,
) -> Result<(), String> {
    if branches.is_empty() {
        return Err("an empty combinator matches nothing it could mean; remove it".to_owned());
    }
    if negate {
        sql.push_str("NOT ");
    }
    sql.push('(');
    for (index, branch) in branches.iter().enumerate() {
        if index > 0 {
            sql.push(' ');
            sql.push_str(joiner);
            sql.push(' ');
        }
        let (branch_sql, branch_params) = where_clause(branch, shape, building)?;
        sql.push_str(&branch_sql);
        params.extend(branch_params);
    }
    sql.push(')');
    Ok(())
}

/// The WHERE clause for one predicate tree: sql over slot columns plus
/// the parameters in placeholder order. An empty tree selects
/// everything (`Class:list {}` is every instance).
///
/// # Errors
/// Refuses unknown fields, building fields, over-cap in-lists and
/// empty combinators, each naming the offender.
pub fn where_clause(
    where_: &Where,
    shape: &Shape,
    building: &HashSet<String>,
) -> Result<(String, Vec<Value>), String> {
    if where_.0.is_empty() {
        return Ok(("1 = 1".to_owned(), Vec::new()));
    }
    let mut sql = String::new();
    let mut params = Vec::new();
    sql.push('(');
    for (index, condition) in where_.0.iter().enumerate() {
        if index > 0 {
            sql.push_str(" AND ");
        }
        render(condition, shape, building, &mut sql, &mut params)?;
    }
    sql.push(')');
    Ok((sql, params))
}

/// The ORDER BY clause, multi-key. Absent values sort last either
/// direction, so present data leads every page.
///
/// # Errors
/// Same refusals as [`where_clause`].
pub fn order_clause(
    order: &[Order],
    shape: &Shape,
    building: &HashSet<String>,
) -> Result<String, String> {
    if order.is_empty() {
        return Ok(String::new());
    }
    let mut keys = Vec::with_capacity(order.len());
    for entry in order {
        // Arrays have no meaningful order; resolve_single's refusal
        // covers ordering too.
        let column = resolve_single(&entry.field, shape, building)?;
        let direction = if entry.descending { "DESC" } else { "ASC" };
        keys.push(format!("{column} IS NULL, {column} {direction}"));
    }
    Ok(format!("ORDER BY {}", keys.join(", ")))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::super::shape::{FieldKind, Shape, Value};
    use super::{Compare, Condition, IN_LIST_CAP, Order, Where, order_clause, where_clause};

    fn shape() -> Shape {
        Shape::declare(&[
            ("status".to_owned(), FieldKind::Single),
            ("closes_at".to_owned(), FieldKind::Single),
            ("tags".to_owned(), FieldKind::Many),
        ])
        .unwrap()
    }

    fn none() -> HashSet<String> {
        HashSet::new()
    }

    #[test]
    fn conditions_land_on_slots_with_bound_params() {
        let tree = Where(vec![
            Condition::Compare {
                field: "status".into(),
                op: Compare::Eq,
                value: Value::Text("open".into()),
            },
            Condition::Compare {
                field: "closes_at".into(),
                op: Compare::Lt,
                value: Value::Number(9000.0),
            },
        ]);
        let (sql, params) = where_clause(&tree, &shape(), &none()).unwrap();
        assert_eq!(sql, "(f1 = ? AND f2 < ?)");
        assert_eq!(params.len(), 2);
        // The value text never appears in the sql: parameters only.
        assert!(!sql.contains("open"));
    }

    #[test]
    fn a_hostile_value_stays_a_parameter() {
        let tree = Where(vec![Condition::Compare {
            field: "status".into(),
            op: Compare::Eq,
            value: Value::Text("'; DROP TABLE rows; --".into()),
        }]);
        let (sql, _) = where_clause(&tree, &shape(), &none()).unwrap();
        assert_eq!(sql, "(f1 = ?)");
    }

    #[test]
    fn unknown_and_building_fields_refuse_by_name() {
        let tree = Where(vec![Condition::Compare {
            field: "statsu".into(),
            op: Compare::Eq,
            value: Value::Text("open".into()),
        }]);
        let error = where_clause(&tree, &shape(), &none()).unwrap_err();
        assert!(error.contains("statsu"), "{error}");

        let building: HashSet<String> = ["status".to_owned()].into();
        let tree = Where(vec![Condition::Compare {
            field: "status".into(),
            op: Compare::Eq,
            value: Value::Text("open".into()),
        }]);
        let error = where_clause(&tree, &shape(), &building).unwrap_err();
        assert!(error.contains("building"), "{error}");
    }

    #[test]
    fn combinators_nest_and_negate() {
        let tree = Where(vec![Condition::Any(vec![
            Where(vec![Condition::Compare {
                field: "closes_at".into(),
                op: Compare::Lt,
                value: Value::Number(100.0),
            }]),
            Where(vec![Condition::None(vec![Where(vec![
                Condition::Compare {
                    field: "status".into(),
                    op: Compare::Eq,
                    value: Value::Text("closed".into()),
                },
            ])])]),
        ])]);
        let (sql, params) = where_clause(&tree, &shape(), &none()).unwrap();
        assert_eq!(sql, "(((f2 < ?) OR (NOT ((f1 = ?)))))");
        assert_eq!(params.len(), 2);
    }

    #[test]
    fn in_lists_cap_and_empty_matches_nothing() {
        let over: Vec<Value> = (0..=IN_LIST_CAP).map(|i| Value::Number(i as f64)).collect();
        let tree = Where(vec![Condition::In {
            field: "closes_at".into(),
            values: over,
        }]);
        assert!(where_clause(&tree, &shape(), &none()).is_err());

        let tree = Where(vec![Condition::In {
            field: "status".into(),
            values: vec![],
        }]);
        let (sql, params) = where_clause(&tree, &shape(), &none()).unwrap();
        assert_eq!(sql, "(1 = 0)");
        assert!(params.is_empty());
    }

    #[test]
    fn contains_walks_the_json_array_with_a_bound_member() {
        let tree = Where(vec![Condition::Contains {
            field: "tags".into(),
            value: Value::Text("vintage".into()),
        }]);
        let (sql, params) = where_clause(&tree, &shape(), &none()).unwrap();
        assert_eq!(
            sql,
            "(EXISTS (SELECT 1 FROM json_each(f3) WHERE json_each.value = ?))"
        );
        assert_eq!(params.len(), 1);
        assert!(!sql.contains("vintage"));
    }

    #[test]
    fn exists_queries_presence_and_absence() {
        let tree = Where(vec![
            Condition::Exists {
                field: "closes_at".into(),
                present: false,
            },
            Condition::Exists {
                field: "tags".into(),
                present: true,
            },
        ]);
        let (sql, params) = where_clause(&tree, &shape(), &none()).unwrap();
        assert_eq!(sql, "(f2 IS NULL AND f3 IS NOT NULL)");
        assert!(params.is_empty());
    }

    #[test]
    fn kinds_gate_their_operators() {
        // Comparing an array field refuses toward its own operators.
        let tree = Where(vec![Condition::Compare {
            field: "tags".into(),
            op: Compare::Eq,
            value: Value::Text("vintage".into()),
        }]);
        let error = where_clause(&tree, &shape(), &none()).unwrap_err();
        assert!(error.contains("contains"), "{error}");

        // Contains on a scalar field refuses toward comparisons.
        let tree = Where(vec![Condition::Contains {
            field: "status".into(),
            value: Value::Text("open".into()),
        }]);
        let error = where_clause(&tree, &shape(), &none()).unwrap_err();
        assert!(error.contains("comparison"), "{error}");

        // Ordering by an array field refuses.
        let error = order_clause(
            &[Order {
                field: "tags".into(),
                descending: false,
            }],
            &shape(),
            &none(),
        )
        .unwrap_err();
        assert!(error.contains("array"), "{error}");
    }

    #[test]
    fn order_sorts_absent_last_and_refuses_unknowns() {
        let clause = order_clause(
            &[
                Order {
                    field: "closes_at".into(),
                    descending: false,
                },
                Order {
                    field: "name".into(),
                    descending: true,
                },
            ],
            &shape(),
            &none(),
        )
        .unwrap();
        assert_eq!(
            clause,
            "ORDER BY f2 IS NULL, f2 ASC, name IS NULL, name DESC"
        );
        assert!(
            order_clause(
                &[Order {
                    field: "nope".into(),
                    descending: false
                }],
                &shape(),
                &none()
            )
            .is_err()
        );
    }
}
