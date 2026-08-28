//! The state surface: the table an object's hooks and methods receive.
//! Plain keys are scratch, `sql` is the durable half, the stream verbs
//! and `set_alarm` ride beside them, and missing keys fall through to
//! the class's own routable methods for direct sibling dispatch.

use std::sync::Arc;

use mlua::{Lua, LuaSerdeExt, Table};

use super::dispatch::{CallChain, CurrentDispatch, ObjectRouter, ObjectTarget, PendingAlarm};
use super::registry::{ClassRegistry, TopicPolicy};
use crate::platform::time::{parse_duration_ms, unix_now_ms};

/// Registry key of the object's state table; exists only in pinned vms,
/// created on their first dispatched call.
const STATE_KEY: &str = "object_state";

/// The identity value gates and `receives` handlers see: name, transport, a
/// canonical id, and `:is(Class)` against a class VALUE, never a string.
pub(super) fn make_identity(
    lua: &Lua,
    class: &str,
    name: &str,
    transport: &str,
) -> mlua::Result<Table> {
    let identity = lua.create_table()?;
    identity.set("__of", class)?;
    identity.set("name", name)?;
    identity.set("transport", transport)?;
    identity.set("id", format!("{class}/{name}"))?;
    identity.set(
        "is",
        lua.create_function(|_, (this, class_handle): (Table, Table)| {
            let mine: String = this.get("__of")?;
            let theirs: String = class_handle.get("__class")?;
            Ok(mine == theirs)
        })?,
    )?;
    Ok(identity)
}

/// `state:set_alarm(duration)`: at most one alarm per object; setting
/// replaces. Persisted alongside the object's rows when storage exists, so
/// it survives a restart once the object is next resident.
fn set_alarm(lua: &Lua, (_this, duration): (Table, mlua::Value)) -> mlua::Result<()> {
    let delay_ms = match &duration {
        mlua::Value::String(raw) => {
            parse_duration_ms(&raw.to_str()?).map_err(mlua::Error::RuntimeError)?
        }
        mlua::Value::Integer(seconds) => seconds * 1000,
        mlua::Value::Number(seconds) => (seconds * 1000.0) as i64,
        _ => {
            return Err(mlua::Error::RuntimeError(
                "set_alarm takes a duration: \"30s\", \"24h\" or seconds.".to_owned(),
            ));
        }
    };

    let (class, name) = lua
        .app_data_ref::<CurrentDispatch>()
        .map(|current| (current.class.clone(), current.name.clone()))
        .ok_or_else(|| {
            mlua::Error::RuntimeError("set_alarm only works inside an object method.".to_owned())
        })?;
    let own_key = lua
        .app_data_ref::<CallChain>()
        .and_then(|chain| chain.0.last().cloned())
        .unwrap_or_default();

    let alarm = PendingAlarm {
        due_ms: unix_now_ms() + delay_ms,
        class,
        name,
        own_key,
    };

    let Some(home) = lua.app_data_ref::<Arc<crate::objects::ObjectHome>>() else {
        return Err(mlua::Error::RuntimeError(
            "set_alarm only works inside an object method.".to_owned(),
        ));
    };
    home.set_alarm(alarm).map_err(mlua::Error::RuntimeError)?;

    Ok(())
}

/// Sequence-table parameters as plain values for the storage layer.
fn sql_params(lua: &Lua, params: Option<Table>) -> mlua::Result<Vec<serde_json::Value>> {
    let Some(params) = params else {
        return Ok(Vec::new());
    };

    let mut values = Vec::new();
    for value in params.sequence_values::<mlua::Value>() {
        values.push(lua.from_value(value?)?);
    }
    Ok(values)
}

/// Runs one storage operation against this vm's object home.
fn with_storage<T>(
    lua: &Lua,
    operation: impl FnOnce(&mut crate::storage::SqliteStorage) -> Result<T, String>,
) -> mlua::Result<T> {
    let home = lua
        .app_data_ref::<Arc<crate::objects::ObjectHome>>()
        .ok_or_else(|| {
            mlua::Error::RuntimeError("This object has no durable storage.".to_owned())
        })?;
    home.with_storage(operation)
        .map_err(mlua::Error::RuntimeError)
}

/// The `state.sql` handle: exec/query/query_one over the object's own
/// database. Synchronous on purpose: the calls run on the object's pinned
/// task against a local file, and one call owns the vm anyway.
fn sql_surface(lua: &Lua) -> mlua::Result<Table> {
    let sql = lua.create_table()?;

    sql.set(
        "exec",
        lua.create_function(
            |lua, (_this, text, params): (Table, String, Option<Table>)| {
                let params = sql_params(lua, params)?;
                with_storage(lua, |storage| storage.exec(&text, &params))
            },
        )?,
    )?;

    sql.set(
        "query",
        lua.create_function(
            |lua, (_this, text, params): (Table, String, Option<Table>)| {
                let params = sql_params(lua, params)?;
                let rows = with_storage(lua, |storage| storage.query(&text, &params))?;
                // Null columns become Lua NIL, never mlua's truthy
                // null sentinel; same rule as every call boundary.
                lua.to_value_with(
                    &rows,
                    mlua::SerializeOptions::new()
                        .serialize_none_to_null(false)
                        .serialize_unit_to_null(false),
                )
            },
        )?,
    )?;

    sql.set(
        "query_one",
        lua.create_function(
            |lua, (_this, text, params): (Table, String, Option<Table>)| {
                let params = sql_params(lua, params)?;
                let rows = with_storage(lua, |storage| storage.query(&text, &params))?;
                match rows.into_iter().next() {
                    Some(row) => lua.to_value_with(
                        &row,
                        mlua::SerializeOptions::new()
                            .serialize_none_to_null(false)
                            .serialize_unit_to_null(false),
                    ),
                    None => Ok(mlua::Value::Nil),
                }
            },
        )?,
    )?;

    Ok(sql)
}

/// One Lua value as the store's typed pair, kv's encoding exactly:
/// raw text for strings, printed numbers and booleans, json for
/// tables. [`None`] means delete, mirroring kv's nil-deletes.
fn store_encode(lua: &Lua, value: mlua::Value) -> mlua::Result<Option<(&'static str, String)>> {
    Ok(Some(match value {
        mlua::Value::Nil => return Ok(None),
        mlua::Value::Boolean(b) => ("boolean", b.to_string()),
        mlua::Value::Integer(i) => ("integer", i.to_string()),
        mlua::Value::Number(n) => ("number", n.to_string()),
        mlua::Value::String(s) => ("string", s.to_str()?.to_owned()),
        table @ mlua::Value::Table(_) => {
            let json: serde_json::Value = lua.from_value(table)?;
            ("json", json.to_string())
        }
        other => {
            return Err(mlua::Error::RuntimeError(format!(
                "Values of type {} cannot be stored; pass strings, numbers,                  booleans, tables or nil.",
                other.type_name()
            )));
        }
    }))
}

/// One stored pair back as the Lua value its type describes. A row
/// disagreeing with its own type is corrupt storage, reported rather
/// than allowed to abort the call.
fn store_decode(lua: &Lua, kind: &str, value: &str) -> mlua::Result<mlua::Value> {
    let mismatch = |expected: &str| {
        mlua::Error::RuntimeError(format!("Stored value is not a valid {expected}."))
    };
    Ok(match kind {
        "string" => mlua::Value::String(lua.create_string(value)?),
        "integer" => mlua::Value::Integer(value.parse().map_err(|_| mismatch("integer"))?),
        "number" => mlua::Value::Number(value.parse().map_err(|_| mismatch("number"))?),
        "boolean" => mlua::Value::Boolean(value.parse().map_err(|_| mismatch("boolean"))?),
        _ => lua.to_value(
            &serde_json::from_str::<serde_json::Value>(value).map_err(|_| mismatch("json"))?,
        )?,
    })
}

/// The store's home, shared by every verb.
fn store_home(lua: &Lua) -> mlua::Result<Arc<crate::objects::ObjectHome>> {
    lua.app_data_ref::<Arc<crate::objects::ObjectHome>>()
        .map(|home| home.clone())
        .filter(|home| home.has_storage())
        .ok_or_else(|| mlua::Error::RuntimeError("state.store needs object storage.".to_owned()))
}

/// The key-value face over the object's own file: kv's verbs, the
/// call's own transaction, the reserved table as the representation
/// (docs/OBJECT-STATE.md).
fn store_surface(lua: &Lua) -> mlua::Result<Table> {
    use crate::platform::state_store;

    let store = lua.create_table()?;
    store.set(
        "get",
        lua.create_function(|lua, (_this, key): (Table, String)| {
            let home = store_home(lua)?;
            let pair = home
                .with_storage(|storage| state_store::get(storage, &key))
                .map_err(mlua::Error::RuntimeError)?;
            match pair {
                Some(pair) => store_decode(lua, &pair.kind, &pair.value),
                None => Ok(mlua::Value::Nil),
            }
        })?,
    )?;
    store.set(
        "set",
        lua.create_function(|lua, (_this, key, value): (Table, String, mlua::Value)| {
            let home = store_home(lua)?;
            match store_encode(lua, value)? {
                Some((kind, text)) => home
                    .with_storage(|storage| state_store::set(storage, &key, kind, &text))
                    .map_err(mlua::Error::RuntimeError),
                None => home
                    .with_storage(|storage| state_store::delete(storage, &key))
                    .map_err(mlua::Error::RuntimeError),
            }
        })?,
    )?;
    store.set(
        "delete",
        lua.create_function(|lua, (_this, key): (Table, String)| {
            let home = store_home(lua)?;
            home.with_storage(|storage| state_store::delete(storage, &key))
                .map_err(mlua::Error::RuntimeError)
        })?,
    )?;
    store.set(
        "set_batch",
        lua.create_function(|lua, (_this, values): (Table, Table)| {
            let home = store_home(lua)?;
            for pair in values.pairs::<String, mlua::Value>() {
                let (key, value) = pair?;
                match store_encode(lua, value)? {
                    Some((kind, text)) => home
                        .with_storage(|storage| state_store::set(storage, &key, kind, &text))
                        .map_err(mlua::Error::RuntimeError)?,
                    None => home
                        .with_storage(|storage| state_store::delete(storage, &key))
                        .map_err(mlua::Error::RuntimeError)?,
                }
            }
            Ok(())
        })?,
    )?;
    store.set(
        "list",
        lua.create_function(|lua, (_this, options): (Table, Option<Table>)| {
            let home = store_home(lua)?;
            let (prefix, limit, cursor) = match &options {
                Some(table) => (
                    table.get::<Option<String>>("prefix")?.unwrap_or_default(),
                    table
                        .get::<Option<i64>>("limit")?
                        .unwrap_or(state_store::LIST_DEFAULT_LIMIT),
                    table.get::<Option<String>>("cursor")?,
                ),
                None => (String::new(), state_store::LIST_DEFAULT_LIMIT, None),
            };
            if !(1..=state_store::LIST_MAX_LIMIT).contains(&limit) {
                return Err(mlua::Error::RuntimeError(format!(
                    "list limit must be between 1 and {}.",
                    state_store::LIST_MAX_LIMIT
                )));
            }
            let (pairs, next) = home
                .with_storage(|storage| {
                    state_store::list(storage, &prefix, limit, cursor.as_deref())
                })
                .map_err(mlua::Error::RuntimeError)?;

            let entries = lua.create_table()?;
            for (index, pair) in pairs.iter().enumerate() {
                let entry = lua.create_table()?;
                entry.set("key", pair.key.clone())?;
                entry.set("value", store_decode(lua, &pair.kind, &pair.value)?)?;
                entries.set(index + 1, entry)?;
            }
            let page = lua.create_table()?;
            page.set("entries", entries)?;
            if let Some(cursor) = next {
                page.set("cursor", cursor)?;
            }
            Ok(mlua::Value::Table(page))
        })?,
    )?;
    Ok(store)
}

/// Builds (or reuses) the object's state table: plain keys are in-memory
/// and live as long as the pinned vm; `state.sql` is the durable half;
/// the stream verbs ride here beside `set_alarm`.
pub(super) fn object_state(lua: &Lua) -> mlua::Result<(Table, bool)> {
    if let Ok(state) = lua.named_registry_value::<Table>(STATE_KEY) {
        return Ok((state, false));
    }
    let state = lua.create_table()?;
    let stored = lua
        .app_data_ref::<Arc<crate::objects::ObjectHome>>()
        .is_some_and(|home| home.has_storage());
    if stored {
        state.set("sql", sql_surface(lua)?)?;
        state.set("store", store_surface(lua)?)?;
    }
    state.set("now", lua.create_function(|_, ()| Ok(unix_now_ms()))?)?;
    state.set("set_alarm", lua.create_function(set_alarm)?)?;

    // state:destroy(): forget this object once the current call commits
    // and answers. Queued calls are refused; naming the identity again
    // later builds a fresh instance at a higher epoch.
    state.set(
        "destroy",
        lua.create_function(|lua, _this: Table| {
            let home = lua
                .app_data_ref::<Arc<crate::objects::ObjectHome>>()
                .ok_or_else(|| {
                    mlua::Error::RuntimeError("destroy runs inside object methods.".to_owned())
                })?;
            home.request_destroy();
            Ok(())
        })?,
    )?;

    // state:publish(topic, event): append to the event log in this
    // call's transaction; delivery pumps after commit.
    state.set(
        "publish",
        lua.create_function(|lua, (_this, topic, event): (Table, String, mlua::Value)| {
            let current = lua
                .app_data_ref::<CurrentDispatch>()
                .map(|current| (current.class.clone(), current.name.clone()))
                .ok_or_else(|| {
                    mlua::Error::RuntimeError("publish runs inside object methods.".to_owned())
                })?;
            let policy = ClassRegistry::of(lua)
                .spec(&current.0)
                .map(|spec| spec.topic_policy(&topic))
                .unwrap_or(TopicPolicy::Absent);
            if matches!(policy, TopicPolicy::Absent) {
                return Err(mlua::Error::RuntimeError(format!(
                    "Topic '{topic}' is not in this class's publishes."
                )));
            }
            // The stored contract is the tamper check: a bundle whose
            // code grew a topic the publish never recorded fails loudly.
            if let Some(prepared) = lua.app_data_ref::<Arc<crate::runtime::PreparedRevision>>()
                && !prepared.contract_allows_publish(&current.0, &topic)
            {
                return Err(mlua::Error::RuntimeError(format!(
                    "The contract does not record '{}' publishing '{topic}'.",
                    current.0
                )));
            }
            let data: serde_json::Value = lua.from_value(event)?;
            let home = lua
                .app_data_ref::<Arc<crate::objects::ObjectHome>>()
                .map(|home| home.clone())
                .filter(|home| home.has_storage())
                .ok_or_else(|| {
                    mlua::Error::RuntimeError("publish needs object storage.".to_owned())
                })?;
            home.with_storage(|storage| {
                crate::streams::append_event(storage, (&current.0, &current.1), &topic, &data)
            })
            .map_err(mlua::Error::RuntimeError)?;
            home.note_publisher(current.0, current.1);
            Ok(())
        })?,
    )?;

    // state:follow(handle, topic, filter?): I follow the target's topic;
    // the target's gate decides, and the edge lives in the target.
    state.set(
        "follow",
        lua.create_async_function(
            |lua, (_this, target, topic, filter): (Table, Table, String, Option<mlua::Value>)| async move {
                stream_edge_call(&lua, target, "__follow", topic, filter).await
            },
        )?,
    )?;
    state.set(
        "unfollow",
        lua.create_async_function(
            |lua, (_this, target, topic): (Table, Table, String)| async move {
                stream_edge_call(&lua, target, "__unfollow", topic, None).await
            },
        )?,
    )?;

    // state:followers(topic?): who follows me, as identity values.
    state.set(
        "followers",
        lua.create_function(|lua, (_this, topic): (Table, Option<String>)| {
            let home = lua
                .app_data_ref::<Arc<crate::objects::ObjectHome>>()
                .map(|home| home.clone())
                .filter(|home| home.has_storage())
                .ok_or_else(|| {
                    mlua::Error::RuntimeError("followers needs object storage.".to_owned())
                })?;
            let edges = home
                .with_storage(|storage| crate::streams::list_edges(storage, topic.as_deref()))
                .map_err(mlua::Error::RuntimeError)?;
            let list = lua.create_table()?;
            for (index, edge) in edges.iter().enumerate() {
                let transport = if edge.kind == "connection" {
                    "connection"
                } else {
                    "object"
                };
                let identity = make_identity(lua, &edge.class, &edge.name, transport)?;
                identity.set("topic", edge.topic.as_str())?;
                list.set(index + 1, identity)?;
            }
            Ok(list)
        })?,
    )?;

    // state:drop_followers(identity): every edge that identity holds
    // here dies; the usual argument is a handle, `Account(user)`.
    state.set(
        "drop_followers",
        lua.create_function(|lua, (_this, target): (Table, Table)| {
            let class: String = target
                .get("__class")
                .or_else(|_| target.get("class"))
                .map_err(|_| {
                    mlua::Error::RuntimeError(
                        "drop_followers takes a handle or { class, name }.".to_owned(),
                    )
                })?;
            let name: String = target
                .get("__name")
                .or_else(|_| target.get("name"))
                .map_err(|_| {
                    mlua::Error::RuntimeError(
                        "drop_followers takes a handle or { class, name }.".to_owned(),
                    )
                })?;
            let home = lua
                .app_data_ref::<Arc<crate::objects::ObjectHome>>()
                .map(|home| home.clone())
                .filter(|home| home.has_storage())
                .ok_or_else(|| {
                    mlua::Error::RuntimeError("drop_followers needs object storage.".to_owned())
                })?;
            home.with_storage(|storage| crate::streams::drop_identity(storage, &class, &name))
                .map_err(mlua::Error::RuntimeError)?;
            Ok(())
        })?,
    )?;

    // `state:method(...)` reaches the class's own routable methods as a
    // DIRECT call: inside a dispatch this vm already is the single
    // writer, so sibling behavior needs no mailbox ride (a handle
    // self-call is a refused cycle) and no hoisted-helper idiom.
    // Resolved per call through the dispatch, because one state table
    // serves every class in the vm; real state keys (platform verbs,
    // user scratch) shadow this fallthrough, hooks and reserved names
    // stay out through the same gate handles use, and non-function
    // class keys (publishes, migrations) stay invisible.
    let fallthrough = lua.create_function(|lua, (_state, key): (Table, String)| {
        let Some(class_name) = lua
            .app_data_ref::<CurrentDispatch>()
            .map(|current| current.class.clone())
        else {
            return Ok(mlua::Value::Nil);
        };
        // The spec already knows which keys are routable methods, so
        // the table is only touched for a name it vouches for.
        let registry = ClassRegistry::of(lua);
        let is_method = registry
            .spec(&class_name)
            .map(|spec| spec.methods.contains(&key))
            .unwrap_or(false);
        if !is_method {
            return Ok(mlua::Value::Nil);
        }
        let Some(class) = registry.class_table(&class_name) else {
            return Ok(mlua::Value::Nil);
        };
        match class.get::<mlua::Value>(key.as_str()) {
            Ok(mlua::Value::Function(method)) => Ok(mlua::Value::Function(method)),
            _ => Ok(mlua::Value::Nil),
        }
    })?;
    let meta = lua.create_table()?;
    meta.set("__index", fallthrough)?;
    state.set_metatable(Some(meta))?;

    lua.set_named_registry_value(STATE_KEY, state.clone())?;
    Ok((state, true))
}

/// Routes a follow or unfollow to the target object as an internal
/// platform verb; the follower is this dispatch's own identity.
async fn stream_edge_call(
    lua: &Lua,
    target: Table,
    verb: &str,
    topic: String,
    filter: Option<mlua::Value>,
) -> mlua::Result<()> {
    let target_class: String = target
        .get("__class")
        .map_err(|_| mlua::Error::RuntimeError("follow takes an object handle.".to_owned()))?;
    let target_name: String = target.get("__name").map_err(|_| {
        mlua::Error::RuntimeError("follow takes an INSTANCE handle, e.g. Channel(id).".to_owned())
    })?;
    let current = lua
        .app_data_ref::<CurrentDispatch>()
        .map(|current| (current.class.clone(), current.name.clone()))
        .ok_or_else(|| {
            mlua::Error::RuntimeError("follow runs inside object methods.".to_owned())
        })?;
    let filter_json: serde_json::Value = match filter {
        Some(value) => lua.from_value(value)?,
        None => serde_json::Value::Null,
    };

    let (router, chain) = {
        let Some(router) = lua.app_data_ref::<ObjectRouter>() else {
            return Err(mlua::Error::RuntimeError(
                "Objects are not available in this runtime.".to_owned(),
            ));
        };
        let chain = lua
            .app_data_ref::<CallChain>()
            .map(|chain| chain.0.clone())
            .unwrap_or_default();
        (router.clone(), chain)
    };

    // Where this follower lives, so the publisher's pump can batch
    // its deliveries into this node's one call.
    let node = lua
        .app_data_ref::<crate::streams::LocalNode>()
        .map(|node| node.0.clone())
        .unwrap_or_default();
    router(ObjectTarget {
        class: target_class,
        name: target_name,
        method: verb.to_owned(),
        arguments: vec![
            serde_json::json!(topic),
            filter_json,
            serde_json::json!({
                "class": current.0,
                "name": current.1,
                "transport": "object",
                "node": node,
            }),
        ],
        chain,
        caller: None,
    })
    .await
    .map_err(mlua::Error::RuntimeError)?;
    Ok(())
}
