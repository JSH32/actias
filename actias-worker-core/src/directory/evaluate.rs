//! Running a class's directory plan and turning what it produces into
//! kernel values. Two rules the rest of the design leans on live here:
//! the plan is read-only (so backfill and verification can run it
//! against any restored copy, no lease, no mailbox), and every failure
//! is contained (the business write commits regardless, the last good
//! row survives, the failure is marked and loud).

use actias_declarations::field_kit::DirectoryPlan;
use mlua::Table;

use super::shape::Value;

/// Largest array a field may hold. An array rides one row like every
/// other value and pays the row byte cap; this bound refuses the
/// pathological case before the encoder builds the text.
pub const MAX_ARRAY_LEN: usize = 256;

/// One evaluated row: flat `(name, value)` pairs in field-name order,
/// which is the order [`super::row::record`] stores and the order the
/// shape's declaration list is derived from.
pub type Row = Vec<(String, Value)>;

/// Turns one Lua value into a field value.
///
/// Integral numbers in i64 range become [`Value::Integer`] so they
/// store and compare exactly; guest numbers are f64, so anything past
/// 2^53 already lost precision before it arrived here and a text field
/// is the honest home for such an id.
fn scalar(value: &mlua::Value, field: &str) -> Result<Option<Value>, String> {
    Ok(Some(match value {
        // A nil is an absent field, not a stored null: a Lua table
        // cannot hold a nil value, so this only ever arrives from an
        // explicit read that found nothing.
        mlua::Value::Nil => return Ok(None),
        mlua::Value::Boolean(flag) => Value::Bool(*flag),
        mlua::Value::Integer(number) => Value::Integer(*number),
        mlua::Value::Number(number) => {
            let integral =
                number.fract() == 0.0 && *number >= i64::MIN as f64 && *number <= i64::MAX as f64;
            if integral {
                Value::Integer(*number as i64)
            } else {
                Value::Number(*number)
            }
        }
        mlua::Value::String(text) => Value::Text(
            text.to_str()
                .map_err(|_| format!("directory field '{field}' holds a string that is not utf-8"))?
                .to_string(),
        ),
        other => {
            return Err(format!(
                "directory field '{field}' is a {}; fields hold strings, numbers, \
                 booleans, or arrays of those",
                other.type_name()
            ));
        }
    }))
}

/// Turns one Lua value into a field value, as the kind the field was
/// declared with expects the shape to be read.
///
/// The kind decides shape, never content: a table under a field
/// declared `array` is a sequence even when it is empty, and any other
/// table is a value no field may hold. Content mismatches (a string
/// where an integer was declared) belong to [`conform`], which is the
/// one place that compares a value against its declaration.
fn field_value(value: &mlua::Value, field: &str, kind: &str) -> Result<Option<Value>, String> {
    let mlua::Value::Table(table) = value else {
        return scalar(value, field);
    };
    if kind != "array" && table.raw_len() == 0 {
        // A record or an empty table under a scalar field: `scalar`
        // owns the message that names what a field may hold.
        return scalar(value, field);
    }

    let mut members = Vec::new();
    for member in table.clone().sequence_values::<mlua::Value>() {
        let member = member.map_err(|error| error.to_string())?;
        if members.len() == MAX_ARRAY_LEN {
            return Err(format!(
                "directory field '{field}' holds more than {MAX_ARRAY_LEN} members"
            ));
        }
        match scalar(&member, field)? {
            Some(value) => members.push(value),
            // A nil inside a sequence ends it in Lua terms;
            // sequence_values already stops there, so this is
            // unreachable in practice and skipped rather than turned
            // into a hole.
            None => continue,
        }
    }
    Ok(Some(Value::Array(members)))
}

/// Reads a bare field's own name from what `from` returned.
fn named(source: &mlua::Value, field: &str) -> Result<mlua::Value, String> {
    let mlua::Value::Table(table) = source else {
        return Err(format!(
            "directory field '{field}' reads its own name from what `from` \
             returned, which is a {}; return a table from `from`, or give \
             the field an extractor",
            source.type_name()
        ));
    };
    table
        .get(field)
        .map_err(|error| format!("directory field '{field}': {error}"))
}

/// Runs a class's directory plan against `state`.
///
/// `from` runs once and every field reads what it returned: a bare
/// marker by the field's own name, a called one through its extractor.
/// A `from` that answers nothing (a fresh object whose row is not
/// written yet) derives an empty row rather than a failure, because
/// absence is legal on every field.
///
/// # Errors
/// Returns the message to mark and log: a throw inside `from` or an
/// extractor, a value no field may hold, or an array past the bound
/// above. Every one of these is contained by the caller; none may fail
/// the call that triggered it.
pub fn evaluate(plan: &DirectoryPlan, state: &Table) -> Result<Row, String> {
    let source: mlua::Value = plan
        .from
        .call(state.clone())
        .map_err(|error| format!("directory `from`: {error}"))?;
    if source.is_nil() {
        return Ok(Row::new());
    }

    // The plan's fields are sorted by name, which is the order
    // [`super::row::record`] stores: the same state must always produce
    // the same row, or every evaluation would look like a change.
    let mut row = Row::new();
    for field in &plan.fields {
        let value = match &field.extract {
            Some(extract) => extract
                .call((source.clone(), state.clone()))
                .map_err(|error| format!("directory field '{}': {error}", field.name))?,
            None => named(&source, &field.name)?,
        };
        if let Some(value) = field_value(&value, &field.name, &field.kind)? {
            row.push((field.name.clone(), value));
        }
    }
    Ok(row)
}

/// Checks a derived row against its publish's declared field set.
///
/// A row may carry any subset of the declaration (nil is absent, and
/// absence is legal on any field), but never a field the publish did
/// not declare, and never a value of a different kind than declared.
/// Refusing here is what makes declared kinds trustworthy everywhere
/// downstream: the overlay binds columns by declared kind, and the
/// verified read compares values by it. A failed conformance is
/// contained exactly like a throw: the business write commits, the row
/// keeps its last good value, the failure is marked.
///
/// An integer conforms to a declared `number` (5 is a number; Lua
/// itself does not distinguish), but a fractional number never
/// conforms to a declared `integer`: that is a real value the declared
/// kind cannot hold, worth naming rather than truncating.
///
/// # Errors
/// Names the offending field and why.
pub fn conform(
    row: &Row,
    spec: &actias_common::directory_spec::DirectorySpec,
) -> Result<(), String> {
    for (name, value) in row {
        let declared = spec
            .fields
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, kind)| kind.as_str());
        let Some(declared) = declared else {
            return Err(format!(
                "directory field '{name}' is not declared; declare it in the class's directory fields or stop returning it"
            ));
        };
        let actual = match value {
            Value::Text(_) => "string",
            Value::Integer(_) => "integer",
            Value::Number(_) => "number",
            Value::Bool(_) => "boolean",
            Value::Array(_) => "array",
        };
        let conforms = actual == declared || (actual == "integer" && declared == "number");
        if !conforms {
            return Err(format!(
                "directory field '{name}' is declared {declared} but the row carries {actual}"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod conformance {
    use super::*;
    use actias_common::directory_spec::DirectorySpec;

    fn spec() -> DirectorySpec {
        DirectorySpec::new(
            1,
            vec![
                ("high_bid".to_owned(), "integer".to_owned()),
                ("ratio".to_owned(), "number".to_owned()),
                ("state".to_owned(), "string".to_owned()),
                ("tags".to_owned(), "array".to_owned()),
            ],
        )
    }

    #[test]
    fn a_subset_of_the_declaration_conforms() {
        // Absence is legal on any field: a nil IS absent, and an
        // auction with no bids yet simply has no high_bid.
        let row: Row = vec![("state".to_owned(), Value::Text("open".to_owned()))];
        assert_eq!(conform(&row, &spec()), Ok(()));
        assert_eq!(conform(&Vec::new(), &spec()), Ok(()));
    }

    #[test]
    fn an_undeclared_field_is_refused_by_name() {
        let row: Row = vec![("winner".to_owned(), Value::Text("ada".to_owned()))];
        let error = conform(&row, &spec()).expect_err("refuses");
        assert!(error.contains("winner"), "{error}");
    }

    #[test]
    fn a_value_of_the_wrong_kind_is_refused() {
        // The overlay binds this column as an integer, so storing text
        // in it would make `high_bid > 100` compare as text and answer
        // on "75" > "100". Refusing keeps the last good row instead.
        let row: Row = vec![("high_bid".to_owned(), Value::Text("lots".to_owned()))];
        let error = conform(&row, &spec()).expect_err("refuses");
        assert!(error.contains("integer"), "{error}");
    }

    #[test]
    fn an_integer_conforms_to_a_declared_number() {
        // Lua does not distinguish them, so a derive returning 5 for a
        // field declared `number` is right, not a mistake.
        let row: Row = vec![("ratio".to_owned(), Value::Integer(5))];
        assert_eq!(conform(&row, &spec()), Ok(()));

        // The reverse is a real value the declared kind cannot hold.
        let row: Row = vec![("high_bid".to_owned(), Value::Number(1.5))];
        assert!(conform(&row, &spec()).is_err());
    }

    #[test]
    fn an_array_and_a_scalar_are_not_interchangeable() {
        let row: Row = vec![("tags".to_owned(), Value::Text("vintage".to_owned()))];
        assert!(conform(&row, &spec()).is_err());
        let row: Row = vec![(
            "state".to_owned(),
            Value::Array(vec![Value::Text("open".to_owned())]),
        )];
        assert!(conform(&row, &spec()).is_err());
    }
}

#[cfg(test)]
mod tests {
    use mlua::Lua;

    use super::super::shape::Value;
    use super::{MAX_ARRAY_LEN, evaluate};

    /// Runs a `directory` table written as Lua source, against an empty
    /// state table, through the same plan reader publish uses.
    fn run(directory: &str) -> Result<Vec<(String, Value)>, String> {
        let lua = Lua::new();
        actias_declarations::field_kit::install(&lua).expect("the kit installs");
        let table: mlua::Table = lua
            .load(format!("return {directory}"))
            .eval()
            .expect("the declaration compiles");
        let plan = actias_declarations::field_kit::plan("Auction", &table)
            .map_err(|error| error.to_string())?;
        let state = lua.create_table().expect("state table");
        evaluate(&plan, &state)
    }

    #[test]
    fn bare_markers_read_their_own_names_and_carry_their_kinds() {
        let row = run(r#"{
                from = function(state)
                    return { status = "open", high_bid = 25, score = 0.5, featured = true }
                end,
                fields = {
                    status = f.string,
                    high_bid = f.integer,
                    score = f.number,
                    featured = f.boolean,
                },
            }"#)
        .unwrap();
        assert_eq!(
            row,
            vec![
                ("featured".to_owned(), Value::Bool(true)),
                ("high_bid".to_owned(), Value::Integer(25)),
                ("score".to_owned(), Value::Number(0.5)),
                ("status".to_owned(), Value::Text("open".to_owned())),
            ],
            "fields sort by name, and an integral number stays an integer"
        );
    }

    #[test]
    fn an_extractor_runs_against_what_from_returned() {
        // The shared read happens once; each field projects from it,
        // which is the whole reason `from` exists.
        let row = run(r#"{
                from = function(state) return { high_bid = "25", bids = { 1, 2 } } end,
                fields = {
                    high_bid = f.integer(function(lot) return tonumber(lot.high_bid) end),
                    bidders = f.integer(function(lot) return #lot.bids end),
                },
            }"#)
        .unwrap();
        assert_eq!(
            row,
            vec![
                ("bidders".to_owned(), Value::Integer(2)),
                ("high_bid".to_owned(), Value::Integer(25)),
            ]
        );
    }

    #[test]
    fn field_order_is_stable_across_evaluations() {
        // Lua hash order is not insertion order and can differ between
        // runs; a row whose field order wandered would look like a
        // change on every write.
        let source = r#"{
            from = function(state) return { zebra = 1, alpha = 2, middle = 3, beta = 4 } end,
            fields = {
                zebra = f.integer, alpha = f.integer, middle = f.integer, beta = f.integer,
            },
        }"#;
        let first = run(source).unwrap();
        let second = run(source).unwrap();
        assert_eq!(first, second);
        let names: Vec<_> = first.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "beta", "middle", "zebra"]);
    }

    #[test]
    fn an_array_field_takes_a_sequence_even_when_it_is_empty() {
        // The declared kind decides the shape: an empty array is an
        // empty array, where guessing from the value alone could only
        // read it as a record and drop the field.
        let row = run(r#"{
                from = function(state) return { tags = { "vintage", "rare" }, seen = {} } end,
                fields = { tags = f.array, seen = f.array },
            }"#)
        .unwrap();
        assert_eq!(
            row,
            vec![
                ("seen".to_owned(), Value::Array(Vec::new())),
                (
                    "tags".to_owned(),
                    Value::Array(vec![
                        Value::Text("vintage".to_owned()),
                        Value::Text("rare".to_owned()),
                    ])
                ),
            ]
        );
    }

    #[test]
    fn absent_fields_are_absent_not_null() {
        let row = run(r#"{
                from = function(state) return { status = "open" } end,
                fields = { status = f.string, closes_at = f.integer },
            }"#)
        .unwrap();
        assert_eq!(
            row,
            vec![("status".to_owned(), Value::Text("open".to_owned()))]
        );
    }

    #[test]
    fn a_from_that_answers_nothing_is_an_empty_row() {
        // A fresh object whose row is not written yet: every field is
        // absent, which is legal, rather than an error nobody caused.
        assert_eq!(
            run(r#"{ from = function(state) return nil end, fields = { status = f.string } }"#)
                .unwrap(),
            Vec::new()
        );
    }

    #[test]
    fn a_throw_is_an_error_not_a_panic() {
        let error =
            run(r#"{ from = function(state) error("boom") end, fields = { status = f.string } }"#)
                .unwrap_err();
        assert!(error.contains("boom"), "{error}");

        let error = run(r#"{
                from = function(state) return {} end,
                fields = { status = f.string(function(lot) error("nope") end) },
            }"#)
        .unwrap_err();
        assert!(
            error.contains("nope") && error.contains("status"),
            "{error}"
        );
    }

    #[test]
    fn unsupported_values_refuse_by_name() {
        let error = run(r#"{
                from = function(state) return { handler = function() end } end,
                fields = { handler = f.string },
            }"#)
        .unwrap_err();
        assert!(error.contains("handler"), "{error}");

        // A bare marker on a source that is not a table cannot read
        // anything, and says which field it was.
        let error = run(r#"{
                from = function(state) return "just a name" end,
                fields = { status = f.string },
            }"#)
        .unwrap_err();
        assert!(
            error.contains("status") && error.contains("`from`"),
            "{error}"
        );
    }

    #[test]
    fn the_array_bound_refuses() {
        let error = run(&format!(
            r#"{{
                from = function(state)
                    local t = {{}} for i = 1, {} do t[i] = i end return {{ tags = t }}
                end,
                fields = {{ tags = f.array }},
            }}"#,
            MAX_ARRAY_LEN + 1
        ))
        .unwrap_err();
        assert!(error.contains("members"), "{error}");
    }
}
