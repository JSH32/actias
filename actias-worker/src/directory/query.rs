//! Translating a wire query into the kernel's predicate tree.
//!
//! The wire carries a tree of field names, operators and json values,
//! never sql: the worker is what turns it into parameterized statements
//! over generated column names, so no caller can shape an identifier.
//! An unknown operator is refused rather than dropped, so a newer
//! caller never silently loses a filter and gets a wider answer than it
//! asked for.

use actias_worker_core::directory::predicate::{Compare, Condition, Where};
use actias_worker_core::directory::shape::Value;
use actias_worker_core::proto::worker_data::{DirectoryCondition, DirectoryWhere};

/// One json value as a field value. Integral numbers become integers so
/// they compare exactly, matching what the evaluation layer stores.
pub(crate) fn value_from_json(raw: &str, field: &str) -> Result<Value, String> {
    let parsed: serde_json::Value = serde_json::from_str(raw)
        .map_err(|_| format!("'{field}' carries a value that is not json"))?;
    json_to_value(&parsed, field)
}

fn json_to_value(parsed: &serde_json::Value, field: &str) -> Result<Value, String> {
    Ok(match parsed {
        serde_json::Value::String(text) => Value::Text(text.clone()),
        serde_json::Value::Bool(flag) => Value::Bool(*flag),
        serde_json::Value::Number(number) => match number.as_i64() {
            Some(integer) => Value::Integer(integer),
            None => Value::Number(number.as_f64().unwrap_or_default()),
        },
        serde_json::Value::Array(members) => Value::Array(
            members
                .iter()
                .map(|member| json_to_value(member, field))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        serde_json::Value::Null => {
            return Err(format!(
                "'{field}' compares against null; absence is the exists operator"
            ));
        }
        other => return Err(format!("'{field}' carries an unsupported value: {other}")),
    })
}

/// The field value as json, for the wire back.
pub fn value_to_json(value: &Value) -> String {
    fn json(value: &Value) -> serde_json::Value {
        match value {
            Value::Text(text) => serde_json::Value::String(text.clone()),
            Value::Integer(number) => (*number).into(),
            Value::Number(number) => serde_json::Number::from_f64(*number)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null),
            Value::Bool(flag) => (*flag).into(),
            Value::Array(members) => serde_json::Value::Array(members.iter().map(json).collect()),
        }
    }
    json(value).to_string()
}

fn condition_from_proto(wire: &DirectoryCondition) -> Result<Condition, String> {
    let field = wire.field.clone();
    let compare = |op: Compare| -> Result<Condition, String> {
        Ok(Condition::Compare {
            value: value_from_json(&wire.value_json, &field)?,
            field: field.clone(),
            op,
        })
    };

    match wire.op.as_str() {
        "eq" => compare(Compare::Eq),
        "ne" => compare(Compare::Ne),
        "lt" => compare(Compare::Lt),
        "lte" => compare(Compare::Lte),
        "gt" => compare(Compare::Gt),
        "gte" => compare(Compare::Gte),
        // "one_of", not "in": the lua surface cannot spell `in` (it is
        // a keyword), and the wire uses the same word the author types.
        "one_of" => {
            let Value::Array(values) = value_from_json(&wire.value_json, &field)? else {
                return Err(format!("'{field}' one_of takes a list of values"));
            };
            Ok(Condition::In { field, values })
        }
        "starts_with" => {
            let Value::Text(prefix) = value_from_json(&wire.value_json, &field)? else {
                return Err(format!("'{field}' starts_with takes a string"));
            };
            Ok(Condition::StartsWith { field, prefix })
        }
        "contains" => Ok(Condition::Contains {
            value: value_from_json(&wire.value_json, &field)?,
            field,
        }),
        "exists" => {
            let Value::Bool(present) = value_from_json(&wire.value_json, &field)? else {
                return Err(format!("'{field}' exists takes true or false"));
            };
            Ok(Condition::Exists { field, present })
        }
        other => Err(format!(
            "'{other}' is not a directory operator; a filter this worker \
             cannot apply is refused rather than ignored"
        )),
    }
}

/// The predicate tree a wire query carries. An absent `where` selects
/// everything, which is what a bare listing of a class means.
///
/// # Errors
/// Refuses an unknown operator, a value of the wrong shape for its
/// operator, and json that does not parse.
pub fn where_from_proto(wire: Option<&DirectoryWhere>) -> Result<Where, String> {
    let Some(wire) = wire else {
        return Ok(Where::default());
    };

    let mut conditions = Vec::new();
    for condition in &wire.conditions {
        conditions.push(condition_from_proto(condition)?);
    }
    let branches = |group: &[DirectoryWhere]| -> Result<Vec<Where>, String> {
        group
            .iter()
            .map(|inner| where_from_proto(Some(inner)))
            .collect()
    };
    if !wire.any.is_empty() {
        conditions.push(Condition::Any(branches(&wire.any)?));
    }
    if !wire.all.is_empty() {
        conditions.push(Condition::All(branches(&wire.all)?));
    }
    if !wire.none.is_empty() {
        conditions.push(Condition::None(branches(&wire.none)?));
    }
    Ok(Where(conditions))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn condition(field: &str, op: &str, value: &str) -> DirectoryCondition {
        DirectoryCondition {
            field: field.to_owned(),
            op: op.to_owned(),
            value_json: value.to_owned(),
        }
    }

    #[test]
    fn operators_translate_and_values_keep_their_kinds() {
        let wire = DirectoryWhere {
            conditions: vec![
                condition("status", "eq", "\"open\""),
                condition("high_bid", "gte", "100"),
                condition("tags", "contains", "\"vintage\""),
                condition("closes_at", "exists", "false"),
            ],
            ..Default::default()
        };
        let tree = where_from_proto(Some(&wire)).expect("translates");
        assert_eq!(tree.0.len(), 4);
        assert!(matches!(
            &tree.0[1],
            Condition::Compare {
                value: Value::Integer(100),
                ..
            }
        ));
        assert!(matches!(
            &tree.0[3],
            Condition::Exists { present: false, .. }
        ));
    }

    #[test]
    fn an_unknown_operator_is_refused_rather_than_ignored() {
        // Dropping it would widen the answer silently, which is the one
        // way a filter can be worse than an error.
        let wire = DirectoryWhere {
            conditions: vec![condition("status", "matches", "\"^open\"")],
            ..Default::default()
        };
        let error = where_from_proto(Some(&wire)).expect_err("refuses");
        assert!(error.contains("matches"), "{error}");
    }

    #[test]
    fn combinators_nest() {
        let wire = DirectoryWhere {
            any: vec![
                DirectoryWhere {
                    conditions: vec![condition("status", "eq", "\"open\"")],
                    ..Default::default()
                },
                DirectoryWhere {
                    none: vec![DirectoryWhere {
                        conditions: vec![condition("high_bid", "lt", "10")],
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let tree = where_from_proto(Some(&wire)).expect("translates");
        let Condition::Any(branches) = &tree.0[0] else {
            panic!("the any survived");
        };
        assert_eq!(branches.len(), 2);
        assert!(matches!(&branches[1].0[0], Condition::None(_)));
    }

    #[test]
    fn an_absent_where_lists_everything() {
        assert!(where_from_proto(None).expect("translates").0.is_empty());
    }

    #[test]
    fn null_points_at_the_exists_operator() {
        let wire = DirectoryWhere {
            conditions: vec![condition("closes_at", "eq", "null")],
            ..Default::default()
        };
        let error = where_from_proto(Some(&wire)).expect_err("refuses");
        assert!(error.contains("exists"), "{error}");
    }

    #[test]
    fn values_round_trip_through_json() {
        assert_eq!(value_to_json(&Value::Integer(25)), "25");
        assert_eq!(value_to_json(&Value::Text("open".into())), "\"open\"");
        assert_eq!(
            value_to_json(&Value::Array(vec![
                Value::Text("a".into()),
                Value::Integer(2)
            ])),
            "[\"a\",2]"
        );
    }
}
