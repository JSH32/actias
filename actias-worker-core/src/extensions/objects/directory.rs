//! The reading verbs on a class handle: `list`, `find` and `visit`, the
//! Lua side of a directory query.
//!
//! The vm speaks the query shape and nothing else. Where an answer
//! comes from (this node's overlay, and which files back it) is the
//! worker's business, reached through [`DirectoryLister`] exactly as a
//! routed method call reaches [`super::dispatch::ObjectRouter`].

use std::sync::Arc;

use mlua::{Lua, Table};

use crate::directory::overlay::{Entry, Page};
use crate::directory::predicate::{Compare, Condition, Order, Where};
use crate::directory::shape::Value;
use crate::directory::verify::VisitedPage;

/// One reading, as the vm asks for it.
pub struct DirectoryRequest {
    pub class: String,
    pub where_: Where,
    pub order: Vec<Order>,
    pub limit: i64,
    pub cursor: Option<String>,
    /// Whether every candidate is checked against its own object's
    /// shipping manifest before it is served. The index is a superset
    /// either way; this is what pays a manifest read per candidate to
    /// narrow it.
    pub verified: bool,
}

/// What the worker answers with, matching what the request asked for.
pub enum DirectoryAnswer {
    Listed(Page),
    Visited(VisitedPage),
}

pub type DirectoryListFuture =
    std::pin::Pin<Box<dyn Future<Output = Result<DirectoryAnswer, String>> + Send>>;

/// How a reading leaves the vm. The worker supplies the overlay and the
/// store; the vm only speaks this shape.
pub type DirectoryLister = Arc<dyn Fn(DirectoryRequest) -> DirectoryListFuture + Send + Sync>;

/// Rows a page carries when the caller does not say.
const DEFAULT_LIMIT: i64 = 100;

/// One Lua value as a field value, for a predicate operand.
fn operand(value: &mlua::Value, field: &str) -> mlua::Result<Value> {
    Ok(match value {
        mlua::Value::String(text) => Value::Text(text.to_str()?.to_string()),
        mlua::Value::Integer(number) => Value::Integer(*number),
        mlua::Value::Boolean(flag) => Value::Bool(*flag),
        mlua::Value::Number(number) => {
            if number.fract() == 0.0 && *number >= i64::MIN as f64 && *number <= i64::MAX as f64 {
                Value::Integer(*number as i64)
            } else {
                Value::Number(*number)
            }
        }
        mlua::Value::Table(table) => {
            let mut members = Vec::new();
            for member in table.clone().sequence_values::<mlua::Value>() {
                members.push(operand(&member?, field)?);
            }
            Value::Array(members)
        }
        mlua::Value::Nil => {
            return Err(mlua::Error::RuntimeError(format!(
                "'{field}' compares against nil; a table cannot hold one. \
                 To match an absent field use exists = false."
            )));
        }
        other => {
            return Err(mlua::Error::RuntimeError(format!(
                "'{field}' compares against a {}; fields hold strings, numbers, \
                 booleans, or arrays of those.",
                other.type_name()
            )));
        }
    })
}

/// The conditions one operator table spells: `{ gte = 100, lt = 5000 }`
/// is two conditions on the same field, and they conjoin.
fn operators(field: &str, table: &Table) -> mlua::Result<Vec<Condition>> {
    let mut conditions = Vec::new();
    for pair in table.clone().pairs::<String, mlua::Value>() {
        let (op, value) = pair?;
        let condition = match op.as_str() {
            "eq" => Condition::Compare {
                field: field.to_owned(),
                op: Compare::Eq,
                value: operand(&value, field)?,
            },
            "ne" => Condition::Compare {
                field: field.to_owned(),
                op: Compare::Ne,
                value: operand(&value, field)?,
            },
            "lt" => Condition::Compare {
                field: field.to_owned(),
                op: Compare::Lt,
                value: operand(&value, field)?,
            },
            "lte" => Condition::Compare {
                field: field.to_owned(),
                op: Compare::Lte,
                value: operand(&value, field)?,
            },
            "gt" => Condition::Compare {
                field: field.to_owned(),
                op: Compare::Gt,
                value: operand(&value, field)?,
            },
            "gte" => Condition::Compare {
                field: field.to_owned(),
                op: Compare::Gte,
                value: operand(&value, field)?,
            },
            // "one_of" rather than "in": `in` is a lua keyword, so
            // `{ in = names }` is a syntax error before any of this
            // runs. One spelling, and it has to be one that parses.
            "one_of" => {
                let Value::Array(values) = operand(&value, field)? else {
                    return Err(mlua::Error::RuntimeError(format!(
                        "'{field}' one_of takes a list of values."
                    )));
                };
                Condition::In {
                    field: field.to_owned(),
                    values,
                }
            }
            "starts_with" => {
                let Value::Text(prefix) = operand(&value, field)? else {
                    return Err(mlua::Error::RuntimeError(format!(
                        "'{field}' starts_with takes a string."
                    )));
                };
                Condition::StartsWith {
                    field: field.to_owned(),
                    prefix,
                }
            }
            "contains" => Condition::Contains {
                field: field.to_owned(),
                value: operand(&value, field)?,
            },
            "exists" => {
                let Value::Bool(present) = operand(&value, field)? else {
                    return Err(mlua::Error::RuntimeError(format!(
                        "'{field}' exists takes true or false."
                    )));
                };
                Condition::Exists {
                    field: field.to_owned(),
                    present,
                }
            }
            other => {
                return Err(mlua::Error::RuntimeError(format!(
                    "'{other}' is not a directory operator. Use eq, ne, lt, lte, \
                     gt, gte, one_of, starts_with, contains or exists."
                )));
            }
        };
        conditions.push(condition);
    }
    Ok(conditions)
}

/// One `where` table as a predicate tree.
pub(super) fn parse_where(table: &Table) -> mlua::Result<Where> {
    let mut conditions = Vec::new();
    for pair in table.clone().pairs::<mlua::Value, mlua::Value>() {
        let (key, value) = pair?;
        let Some(field) = key
            .as_string()
            .and_then(|text| text.to_str().ok().map(|text| text.to_string()))
        else {
            continue;
        };

        match field.as_str() {
            "any" | "all" | "none" => {
                let mlua::Value::Table(group) = value else {
                    return Err(mlua::Error::RuntimeError(format!(
                        "'{field}' takes a list of where tables."
                    )));
                };
                let mut branches = Vec::new();
                for branch in group.sequence_values::<Table>() {
                    branches.push(parse_where(&branch?)?);
                }
                conditions.push(match field.as_str() {
                    "any" => Condition::Any(branches),
                    "all" => Condition::All(branches),
                    _ => Condition::None(branches),
                });
            }
            // A table value is an operator table; anything else is the
            // equality shorthand, which is what most filters are.
            _ => match value {
                mlua::Value::Table(operators_table) => {
                    conditions.extend(operators(&field, &operators_table)?);
                }
                other => conditions.push(Condition::Compare {
                    value: operand(&other, &field)?,
                    field,
                    op: Compare::Eq,
                }),
            },
        }
    }
    Ok(Where(conditions))
}

/// The `order` table as sort keys: `{ closes_at = "asc" }`, or a list
/// of `{ field, descending }` pairs is not offered, because the keyed
/// spelling is what the docs teach and one spelling is enough.
fn parse_order(table: &Table) -> mlua::Result<Vec<Order>> {
    let mut order = Vec::new();
    // One key: a Lua table has no order between its keys, so a second
    // sort key's precedence could not be read from it.
    if table.clone().pairs::<String, String>().count() > 1 {
        return Err(mlua::Error::RuntimeError(
            "order takes one field; a table cannot say which of two comes first. \
             Sort by the one that matters and page by the cursor."
                .to_owned(),
        ));
    }
    for pair in table.clone().pairs::<String, String>() {
        let (field, direction) = pair?;
        let descending = match direction.as_str() {
            "asc" => false,
            "desc" => true,
            other => {
                return Err(mlua::Error::RuntimeError(format!(
                    "'{other}' is not a sort direction; use \"asc\" or \"desc\"."
                )));
            }
        };
        order.push(Order { field, descending });
    }
    Ok(order)
}

/// Builds the request one `Class:list { ... }` or `Class:visit { ... }`
/// call means.
pub(super) fn parse_request(
    class: String,
    options: Option<Table>,
    verified: bool,
) -> mlua::Result<DirectoryRequest> {
    let Some(options) = options else {
        return Ok(DirectoryRequest {
            class,
            where_: Where::default(),
            order: Vec::new(),
            limit: DEFAULT_LIMIT,
            cursor: None,
            verified,
        });
    };

    let where_ = match options.get::<Option<Table>>("where")? {
        Some(table) => parse_where(&table)?,
        None => Where::default(),
    };
    let order = match options.get::<Option<Table>>("order")? {
        Some(table) => parse_order(&table)?,
        None => Vec::new(),
    };

    Ok(DirectoryRequest {
        class,
        where_,
        order,
        limit: options
            .get::<Option<i64>>("limit")?
            .unwrap_or(DEFAULT_LIMIT),
        cursor: options.get::<Option<String>>("cursor")?,
        verified,
    })
}

/// Builds the request one `Class:find { ... }` call means: the
/// predicate is the whole argument, and everything else defaults.
///
/// The shorthand exists because a predicate with default order and
/// limit is what most reads are, and `list { where = { ... } }` spends
/// a nesting level on saying so.
pub(super) fn parse_find(
    class: String,
    predicate: Option<Table>,
) -> mlua::Result<DirectoryRequest> {
    Ok(DirectoryRequest {
        class,
        where_: match predicate {
            Some(table) => parse_where(&table)?,
            None => Where::default(),
        },
        order: Vec::new(),
        limit: DEFAULT_LIMIT,
        cursor: None,
        verified: false,
    })
}

/// One page as the Lua surface's `{ entries, cursor }`, the same shape
/// kv and the object store already page with.
///
/// A verified page's entries also carry `unverified`, and `reason`
/// where there is one: the flag is what tells a caller a row was served
/// without proof, which the verified read never hides by dropping it.
pub(super) fn answer_to_lua(lua: &Lua, answer: DirectoryAnswer) -> mlua::Result<Table> {
    let entries = lua.create_table()?;
    let cursor = match answer {
        DirectoryAnswer::Listed(page) => {
            for (index, entry) in page.entries.into_iter().enumerate() {
                entries.set(index + 1, entry_to_lua(lua, entry)?)?;
            }
            page.cursor
        }
        DirectoryAnswer::Visited(page) => {
            for (index, visited) in page.entries.into_iter().enumerate() {
                let entry = entry_to_lua(lua, visited.entry)?;
                entry.set("unverified", visited.unverified)?;
                if let Some(reason) = visited.reason {
                    entry.set("reason", reason)?;
                }
                entries.set(index + 1, entry)?;
            }
            page.cursor
        }
    };

    let page = lua.create_table()?;
    page.set("entries", entries)?;
    if let Some(cursor) = cursor {
        page.set("cursor", cursor)?;
    }
    Ok(page)
}

fn entry_to_lua(lua: &Lua, entry: Entry) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("name", entry.name)?;
    for (field, value) in entry.fields {
        table.set(field, value_to_lua(lua, &value)?)?;
    }
    Ok(table)
}

fn value_to_lua(lua: &Lua, value: &Value) -> mlua::Result<mlua::Value> {
    Ok(match value {
        Value::Text(text) => mlua::Value::String(lua.create_string(text)?),
        Value::Integer(number) => mlua::Value::Integer(*number),
        Value::Number(number) => mlua::Value::Number(*number),
        Value::Bool(flag) => mlua::Value::Boolean(*flag),
        Value::Array(members) => {
            let table = lua.create_table()?;
            for (index, member) in members.iter().enumerate() {
                table.set(index + 1, value_to_lua(lua, member)?)?;
            }
            mlua::Value::Table(table)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str) -> mlua::Result<Where> {
        let lua = Lua::new();
        let table: Table = lua.load(source).eval().expect("the table evaluates");
        parse_where(&table)
    }

    #[test]
    fn a_bare_value_is_equality_and_a_table_is_operators() {
        let tree =
            parse(r#"{ status = "open", high_bid = { gte = 100, lt = 5000 } }"#).expect("parses");
        assert_eq!(tree.0.len(), 3, "one equality and two comparisons");
        assert!(tree.0.iter().any(|condition| matches!(
            condition,
            Condition::Compare { field, op: Compare::Eq, value: Value::Text(text) }
                if field == "status" && text == "open"
        )));
    }

    #[test]
    fn combinators_nest() {
        let tree = parse(
            r#"{ any = { { status = "open" }, { none = { { high_bid = { lt = 10 } } } } } }"#,
        )
        .expect("parses");
        let Condition::Any(branches) = &tree.0[0] else {
            panic!("the any survived");
        };
        assert_eq!(branches.len(), 2);
        assert!(matches!(&branches[1].0[0], Condition::None(_)));
    }

    #[test]
    fn every_operator_spells_its_condition() {
        let tree = parse(
            r#"{
                tags = { contains = "vintage" },
                closes_at = { exists = false },
                seller = { one_of = { "a", "b" } },
                name = { starts_with = "lot:" },
            }"#,
        )
        .expect("parses");
        assert_eq!(tree.0.len(), 4);
        assert!(
            tree.0
                .iter()
                .any(|c| matches!(c, Condition::Contains { .. }))
        );
        assert!(
            tree.0
                .iter()
                .any(|c| matches!(c, Condition::Exists { present: false, .. }))
        );
        assert!(
            tree.0
                .iter()
                .any(|c| matches!(c, Condition::In { values, .. } if values.len() == 2))
        );
        assert!(
            tree.0
                .iter()
                .any(|c| matches!(c, Condition::StartsWith { .. }))
        );
    }

    #[test]
    fn an_unknown_operator_names_the_ones_that_exist() {
        let error = parse(r#"{ status = { matches = "^open" } }"#).expect_err("refuses");
        let message = error.to_string();
        assert!(message.contains("matches"), "{message}");
        assert!(message.contains("starts_with"), "{message}");
    }

    #[test]
    fn integral_numbers_stay_integers() {
        let tree = parse(r#"{ high_bid = 25 }"#).expect("parses");
        assert!(matches!(
            &tree.0[0],
            Condition::Compare {
                value: Value::Integer(25),
                ..
            }
        ));
    }

    #[test]
    fn an_empty_where_lists_everything() {
        assert!(parse("{}").expect("parses").0.is_empty());
    }

    #[test]
    fn a_bad_sort_direction_says_what_is_allowed() {
        let lua = Lua::new();
        let table: Table = lua
            .load(r#"{ closes_at = "ascending" }"#)
            .eval()
            .expect("evaluates");
        let error = parse_order(&table).expect_err("refuses");
        assert!(error.to_string().contains("asc"), "{error}");
    }

    /// A class whose handle the verbs hang off. Global, so a chunk
    /// evaluated after the script can reach it.
    const AUCTION: &str = r#"
        Auction = object "Auction" {
            migrations = "migrations/Auction",
            directory = {
                from = function(state) return state.store:get("lot") end,
                fields = { state = f.string },
            },
        }
        on "fetch" (function() return { body = "ok" } end)
    "#;

    /// What the fake seam recorded, one entry per call.
    type Asked = std::sync::Arc<std::sync::Mutex<Vec<(bool, usize, i64)>>>;

    /// A runtime whose directory seam records what it was asked and
    /// answers a single-row page, verified pages carrying a flagged
    /// row so the flag's trip into Lua is exercised.
    async fn runtime_with_seam() -> (crate::runtime::ActiasRuntime, Asked) {
        let runtime = crate::objects::testing::runtime_with(AUCTION).await;
        let asked: Asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorder = asked.clone();
        let lister: DirectoryLister = std::sync::Arc::new(move |request: DirectoryRequest| {
            recorder
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push((request.verified, request.where_.0.len(), request.limit));
            let entry = Entry {
                name: "lot-a".to_owned(),
                object_id: "id-a".to_owned(),
                fields: vec![("state".to_owned(), Value::Text("open".to_owned()))],
            };
            let answer = if request.verified {
                DirectoryAnswer::Visited(VisitedPage {
                    entries: vec![crate::directory::verify::Visited {
                        entry,
                        unverified: true,
                        reason: Some("nothing has shipped for this object yet".to_owned()),
                    }],
                    cursor: None,
                })
            } else {
                DirectoryAnswer::Listed(Page {
                    entries: vec![entry],
                    cursor: None,
                })
            };
            Box::pin(async move { Ok(answer) })
        });
        runtime.set_app_data::<DirectoryLister>(lister);
        (runtime, asked)
    }

    /// `find` is the predicate alone, `visit` asks for verification,
    /// and `list` asks for neither. Driven through the handle rather
    /// than the parser, because what a verb asks for is the behaviour.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_reading_verbs_ask_for_what_they_promise() {
        let (runtime, asked) = runtime_with_seam().await;

        let name: String = runtime
            .load(r#"return Auction:find({ state = "open" }).entries[1].name"#)
            .eval_async()
            .await
            .expect("find answers");
        assert_eq!(name, "lot-a");

        runtime
            .load(r#"return Auction:list { where = { state = "open" }, limit = 7 }"#)
            .eval_async::<Table>()
            .await
            .expect("list answers");

        let (unverified, reason): (bool, String) = runtime
            .load(
                r#"local page = Auction:visit { where = { state = "open" } }
                   local entry = page.entries[1]
                   return entry.unverified, entry.reason"#,
            )
            .eval_async()
            .await
            .expect("visit answers");
        // Flagged, not dropped: the whole point of the verified read.
        assert!(unverified);
        assert!(reason.contains("shipped"), "{reason}");

        let asked = asked.lock().unwrap_or_else(|p| p.into_inner()).clone();
        assert_eq!(
            asked,
            vec![
                (false, 1, DEFAULT_LIMIT),
                (false, 1, 7),
                (true, 1, DEFAULT_LIMIT)
            ],
            "find defaults its limit, list takes the one it was given, \
             and only visit asks to be verified"
        );
    }
}
