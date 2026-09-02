//! The declared shape of a class's directory row: an ordered field
//! list, validated once at publish. Fields travel by name everywhere
//! durable (the reserved table, deltas, the manifest-carried row), so
//! there is no fixed column set and no cap on field count: the row's
//! honest bound is its encoded size, which is what the shipping path
//! actually pays for. Nested tables in a `directory` return flatten
//! to dotted field names at evaluation (`location.region`); the
//! kernel only ever sees flat pairs.
//!
//! Columns exist only in a node's local overlay, which is rebuilt per
//! generation, so a grown shape is a local rebuild, never a wire or
//! schema change. Overlay column names are generated from the field's
//! declaration index by [`Shape::slot`], never from user text: that
//! closed derivation is the injection-safety argument for every
//! identifier the predicate translator emits.

/// The name rules live with the declaration codec, so the publish pass
/// and this kernel refuse identically: a class that passes one check
/// and fails the other is published broken and only discovers it on
/// its first write.
pub use actias_common::directory_spec::{FIELD_NAME_MAX_BYTES, RESERVED_NAMES, check_field_name};

/// The implicit, always-present field: the instance name. Queryable
/// like a declared field, but stored as the row key, not a slot.
pub const NAME_COLUMN: &str = "name";

/// One field value. The set is closed by a rule: a kind is admitted
/// only with the predicates that are honest for it, which is what
/// separates queryable from merely stored. Scalars take comparison
/// and prefix predicates; arrays take membership (`contains`) and
/// absence (`exists`) only, because arrays have no meaningful total
/// order. Opaque json objects stay out by the same rule: nothing
/// honest can be asked of one, and nesting flattens to dotted fields
/// instead. Scalar kind spellings match the kv service's typed pairs,
/// so one rendering serves kv, object state and the directory alike.
///
/// Integers store exactly; guest Luau numbers are f64, so the
/// evaluation layer stores integral values in range as
/// [`Value::Integer`] and anything past 2^53 has already lost
/// precision in the guest.
///
/// There is no null variant, and there cannot be one: a Lua table
/// cannot hold a nil value, so absence is the only thing the guest
/// can express. An absent field is an absent pair, stored as sql
/// NULL; comparison predicates never match it, and querying *for*
/// absence is the `exists` predicate, not a stored null.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Text(String),
    Integer(i64),
    Number(f64),
    Bool(bool),
    /// Members are scalars only; a nested array or table inside one
    /// is refused at evaluation, not silently flattened.
    Array(Vec<Value>),
}

impl Value {
    /// Whether this value may be an array member.
    pub fn is_scalar(&self) -> bool {
        !matches!(self, Value::Array(_))
    }
}

impl rusqlite::ToSql for Value {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(match self {
            Value::Text(text) => rusqlite::types::ToSqlOutput::Borrowed(text.as_str().into()),
            Value::Integer(number) => (*number).into(),
            Value::Number(number) => (*number).into(),
            Value::Bool(flag) => (*flag as i64).into(),
            // Arrays bind as their json text, the same encoding the
            // overlay column stores, so json_each reads either side.
            Value::Array(_) => {
                rusqlite::types::ToSqlOutput::Owned(super::row::to_json_text(self).into())
            }
        })
    }
}

/// Which predicate family a declared field admits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    /// One scalar: comparisons, `in`, `starts_with`, ordering.
    Single,
    /// An array of scalars: `contains` and `exists` only.
    Many,
}

/// A validated declaration: the fields one dver exposes, in
/// declaration order with their predicate family (recorded at publish
/// from the analyzer's view of the `directory` return type). Rows
/// carry field names, so a later declaration can never reinterpret
/// bytes an older one wrote; dver orders merges, nothing more.
#[derive(Debug, Clone)]
pub struct Shape {
    fields: Vec<(String, FieldKind)>,
}

impl Shape {
    /// Validates a declaration.
    ///
    /// # Errors
    /// Refuses a reserved name, a duplicate, or a name past
    /// [`FIELD_NAME_MAX_BYTES`], each naming the field.
    pub fn declare(declared: &[(String, FieldKind)]) -> Result<Shape, String> {
        let mut fields: Vec<(String, FieldKind)> = Vec::with_capacity(declared.len());
        for (name, kind) in declared {
            check_field_name(name)?;
            if fields.iter().any(|(existing, _)| existing == name) {
                return Err(format!("directory field '{name}' is declared twice"));
            }
            fields.push((name.clone(), *kind));
        }
        Ok(Shape { fields })
    }

    /// The overlay column for a field: [`NAME_COLUMN`] for the
    /// implicit name field, a generated `f{index}` for a declared one,
    /// [`None`] for a field this shape never declared, which callers
    /// surface as the typo refusal. Generated from the declaration
    /// index, never from user text.
    pub fn slot(&self, field: &str) -> Option<String> {
        if field == NAME_COLUMN {
            return Some(NAME_COLUMN.to_owned());
        }
        self.fields
            .iter()
            .position(|(name, _)| name == field)
            .map(|index| format!("f{}", index + 1))
    }

    /// The predicate family of a field; the implicit name field is
    /// [`FieldKind::Single`].
    pub fn kind(&self, field: &str) -> Option<FieldKind> {
        if field == NAME_COLUMN {
            return Some(FieldKind::Single);
        }
        self.fields
            .iter()
            .find(|(name, _)| name == field)
            .map(|(_, kind)| *kind)
    }

    /// The declared fields in declaration order.
    pub fn fields(&self) -> &[(String, FieldKind)] {
        &self.fields
    }
}

#[cfg(test)]
mod tests {
    use super::{FIELD_NAME_MAX_BYTES, FieldKind, NAME_COLUMN, Shape};

    fn declare(names: &[&str]) -> Result<Shape, String> {
        Shape::declare(
            &names
                .iter()
                .map(|n| ((*n).to_owned(), FieldKind::Single))
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn columns_generate_from_declaration_order() {
        let shape = declare(&["status", "high_bid", "seller"]).unwrap();
        assert_eq!(shape.slot("status").as_deref(), Some("f1"));
        assert_eq!(shape.slot("high_bid").as_deref(), Some("f2"));
        assert_eq!(shape.slot("seller").as_deref(), Some("f3"));
        assert_eq!(shape.slot(NAME_COLUMN).as_deref(), Some(NAME_COLUMN));
        assert_eq!(shape.slot("statsu"), None);
    }

    #[test]
    fn kinds_travel_with_fields() {
        let shape = Shape::declare(&[
            ("status".to_owned(), FieldKind::Single),
            ("tags".to_owned(), FieldKind::Many),
        ])
        .unwrap();
        assert_eq!(shape.kind("status"), Some(FieldKind::Single));
        assert_eq!(shape.kind("tags"), Some(FieldKind::Many));
        assert_eq!(shape.kind(NAME_COLUMN), Some(FieldKind::Single));
        assert_eq!(shape.kind("nope"), None);
    }

    #[test]
    fn field_count_is_unbounded() {
        let many: Vec<_> = (0..64)
            .map(|i| (format!("field_{i}"), FieldKind::Single))
            .collect();
        let shape = Shape::declare(&many).unwrap();
        assert_eq!(shape.slot("field_63").as_deref(), Some("f64"));
    }

    #[test]
    fn reserved_duplicate_and_oversized_names_refuse() {
        assert!(declare(&["any"]).is_err());
        assert!(declare(&["name"]).is_err());
        assert!(declare(&["status", "status"]).is_err());
        let long = "f".repeat(FIELD_NAME_MAX_BYTES + 1);
        assert!(Shape::declare(&[(long, FieldKind::Single)]).is_err());
    }
}
