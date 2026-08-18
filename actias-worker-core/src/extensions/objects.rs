//! Durable object declarations and calls.
//!
//! `local Room = object "Room" { ... }` declares a class at the top level
//! and returns its handle; `objects "Room"` references one declared
//! elsewhere. `Room:get("lobby")` mints an instance handle whose method
//! calls travel through the node's object router to the instance's pinned
//! vm, one mailbox message per call, arguments and returns as plain
//! serializable values.
//!
//! Every vm also carries `__dispatch`, the receiving side: it resolves the
//! class method registered in this vm and runs it with the object's state
//! table first. A request vm never dispatches; the pinned vm the router
//! spawned does.

use std::sync::Arc;

use mlua::{Lua, LuaSerdeExt, Table};
use serde::Deserialize;

use crate::runtime::extension::{ExtensionInfo, LuaExtension};
use crate::runtime::{ActiasRuntime, ContractKind};

/// Registry key of the class-name-to-methods table in this vm.
const CLASSES_KEY: &str = "object_classes";

/// The built-in class behind `database "name"`; the `__` prefix is
/// reserved, so no user class can collide with it.
const DATABASE_CLASS: &str = "__database";

/// Registry key of the object's state table; exists only in pinned vms,
/// created on their first dispatched call.
const STATE_KEY: &str = "object_state";

/// One routed method call.
pub struct ObjectTarget {
    pub class: String,
    pub name: String,
    pub method: String,
    pub arguments: Vec<serde_json::Value>,
    /// Object keys already on this call's stack; the router refuses a
    /// target that appears here, because its mailbox is busy underneath
    /// this very call.
    pub chain: Vec<String>,
}

/// The call chain the currently dispatched method arrived on; app data in
/// pinned vms, absent in request vms. One slot suffices because a pinned
/// vm runs exactly one call at a time.
pub struct CallChain(pub Vec<String>);

/// The one alarm an object may hold; setting replaces. The task loop reads
/// it after every call to know when to wake.
pub struct AlarmCell(pub std::cell::RefCell<Option<PendingAlarm>>);

#[derive(Clone)]
pub struct PendingAlarm {
    /// Unix milliseconds the alarm is due at.
    pub due_ms: i64,
    /// Class whose `alarm` method runs.
    pub class: String,
    /// The object's own key, seeding the alarm dispatch's call chain.
    pub own_key: String,
}

/// Milliseconds since the unix epoch, the clock `state.now()` exposes and
/// the alarm loop schedules against.
pub fn unix_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// A duration written the way scripts write them: "500ms", "30s", "10m",
/// "24h", "7d", or a bare number of seconds.
fn parse_duration_ms(raw: &str) -> Result<i64, String> {
    let raw = raw.trim();
    if let Ok(seconds) = raw.parse::<f64>() {
        return Ok((seconds * 1000.0) as i64);
    }

    let split = raw
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .ok_or_else(|| format!("'{raw}' is not a duration."))?;
    let (number, unit) = raw.split_at(split);
    let number: f64 = number
        .parse()
        .map_err(|_| format!("'{raw}' is not a duration."))?;

    let factor = match unit.trim() {
        "ms" => 1.0,
        "s" => 1000.0,
        "m" => 60.0 * 1000.0,
        "h" => 3600.0 * 1000.0,
        "d" => 86400.0 * 1000.0,
        other => return Err(format!("Unknown duration unit '{other}'.")),
    };

    Ok((number * factor) as i64)
}

/// What `__dispatch` receives from the mailbox, mirroring [`ObjectTarget`]
/// minus the name, which the pinned vm embodies rather than reads.
#[derive(Deserialize)]
struct DispatchCall {
    class: String,
    method: String,
    #[serde(default)]
    args: Vec<serde_json::Value>,
    /// The stack this call rides on, own key included, installed for the
    /// method's outbound calls to extend.
    #[serde(default)]
    chain: Vec<String>,
}

/// How a method call leaves this vm: the worker supplies the routing (id
/// derivation, placement, the mailbox); the vm only speaks this shape.
pub type RouterFuture =
    std::pin::Pin<Box<dyn Future<Output = Result<serde_json::Value, String>> + Send>>;
pub type ObjectRouter = Arc<dyn Fn(ObjectTarget) -> RouterFuture + Send + Sync>;

pub struct ObjectExtension;

impl LuaExtension for ObjectExtension {
    fn extension_info(&self) -> ExtensionInfo<'_> {
        ExtensionInfo {
            name: "object",
            description: "Durable object classes with serialized method calls",
            default: true,
        }
    }

    fn create_extension(&self, lua: &mlua::Lua) -> mlua::Result<mlua::Value> {
        let classes = lua.create_table()?;
        classes.set(DATABASE_CLASS, database_class(lua)?)?;
        lua.set_named_registry_value(CLASSES_KEY, classes)?;

        // `database "name"`: the sql product face, sugar over an object of
        // the built-in class; same lease, same file, same mailbox.
        lua.globals().set(
            "database",
            lua.create_function(|lua, name: String| {
                ActiasRuntime::assert_declaration_phase(lua, "database")?;
                ActiasRuntime::assert_contract_allows(lua, ContractKind::Database, &name)?;
                ActiasRuntime::record_database_declaration(lua, &name);
                instance_handle(lua, DATABASE_CLASS.to_owned(), name)
            })?,
        )?;

        // `objects "Class"`: reference a class declared elsewhere. It mints
        // the same handle; whether the class exists is the callee's truth.
        lua.globals().set(
            "objects",
            lua.create_function(|lua, class: String| {
                ActiasRuntime::assert_declaration_phase(lua, "objects")?;
                class_handle(lua, class)
            })?,
        )?;

        install_dispatch(lua)?;

        // `object "Class" { methods }`, curried: the name is checked and
        // recorded, the table becomes the class body in this vm.
        let declaration = lua.create_function(|lua, class: String| {
            ActiasRuntime::assert_declaration_phase(lua, "object")?;
            if class.starts_with("__") {
                return Err(mlua::Error::RuntimeError(format!(
                    "Class name '{class}' is reserved for the platform."
                )));
            }
            ActiasRuntime::assert_contract_allows(lua, ContractKind::Object, &class)?;
            ActiasRuntime::record_object_declaration(lua, &class);

            lua.create_function(move |lua, methods: Table| {
                let classes: Table = lua.named_registry_value(CLASSES_KEY)?;
                classes.set(class.as_str(), methods)?;
                class_handle(lua, class.clone())
            })
        })?;

        Ok(mlua::Value::Function(declaration))
    }
}

/// The class handle: `Class:get(name)` mints an instance handle.
fn class_handle(lua: &Lua, class: String) -> mlua::Result<Table> {
    let handle = lua.create_table()?;
    handle.set("__class", class)?;

    handle.set(
        "get",
        lua.create_function(|lua, (this, name): (Table, String)| {
            let class: String = this.get("__class")?;
            instance_handle(lua, class, name)
        })?,
    )?;

    Ok(handle)
}

/// The instance handle: any method name resolves to a routed call.
fn instance_handle(lua: &Lua, class: String, name: String) -> mlua::Result<Table> {
    let handle = lua.create_table()?;
    handle.set("__class", class)?;
    handle.set("__name", name)?;

    let meta = lua.create_table()?;
    meta.set(
        "__index",
        lua.create_function(|lua, (this, method): (Table, String)| {
            let class: String = this.get("__class")?;
            let name: String = this.get("__name")?;

            lua.create_async_function(move |lua, args: mlua::MultiValue| {
                let class = class.clone();
                let name = name.clone();
                let method = method.clone();

                async move {
                    // The colon-call receiver is the handle itself; what
                    // travels is everything after it, as plain values.
                    let mut values = args.into_iter();
                    let _receiver = values.next();
                    let mut arguments = Vec::new();
                    for value in values {
                        arguments.push(lua.from_value::<serde_json::Value>(value)?);
                    }

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

                    let result = router(ObjectTarget {
                        class,
                        name,
                        method,
                        arguments,
                        chain,
                    })
                    .await
                    .map_err(mlua::Error::RuntimeError)?;

                    lua.to_value(&result)
                }
            })
        })?,
    )?;

    handle.set_metatable(Some(meta))?;
    Ok(handle)
}

/// The built-in database class body: thin lua over `state.sql`, so a
/// database is an ordinary object with the storage surface as its methods.
fn database_class(lua: &Lua) -> mlua::Result<Table> {
    let body = lua.create_table()?;

    body.set(
        "exec",
        lua.create_function(
            |lua, (state, text, params): (Table, String, Option<Table>)| {
                let sql: Table = state.get("sql")?;
                let exec: mlua::Function = sql.get("exec")?;
                exec.call::<u64>((sql, text, params))?;
                lua.to_value(&serde_json::Value::Bool(true))
            },
        )?,
    )?;

    body.set(
        "query",
        lua.create_function(|_, (state, text, params): (Table, String, Option<Table>)| {
            let sql: Table = state.get("sql")?;
            let query: mlua::Function = sql.get("query")?;
            query.call::<mlua::Value>((sql, text, params))
        })?,
    )?;

    // A batch is nothing special: one call is one transaction already, so
    // this is a loop with the atomicity coming from the dispatch guard.
    body.set(
        "batch",
        lua.create_function(|_, (state, statements): (Table, Table)| {
            let sql: Table = state.get("sql")?;
            let exec: mlua::Function = sql.get("exec")?;

            let mut affected = Vec::new();
            for entry in statements.sequence_values::<Table>() {
                let entry = entry?;
                let text: String = entry.get(1)?;
                let params: Option<Table> = entry.get(2)?;
                affected.push(exec.call::<u64>((sql.clone(), text, params))?);
            }
            Ok(affected)
        })?,
    )?;

    body.set(
        "query_one",
        lua.create_function(|_, (state, text, params): (Table, String, Option<Table>)| {
            let sql: Table = state.get("sql")?;
            let query_one: mlua::Function = sql.get("query_one")?;
            query_one.call::<mlua::Value>((sql, text, params))
        })?,
    )?;

    Ok(body)
}

/// Which class the pinned vm is currently dispatching for.
struct CurrentDispatch {
    class: String,
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

    let class = lua
        .app_data_ref::<CurrentDispatch>()
        .map(|current| current.class.clone())
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
        own_key,
    };

    if let Some(cell) = lua.app_data_ref::<crate::storage::StorageCell>() {
        cell.0
            .borrow_mut()
            .save_alarm(alarm.due_ms, &alarm.class, &alarm.own_key)
            .map_err(mlua::Error::RuntimeError)?;
    }

    match lua.app_data_ref::<AlarmCell>() {
        Some(cell) => *cell.0.borrow_mut() = Some(alarm),
        None => {
            lua.set_app_data(AlarmCell(std::cell::RefCell::new(Some(alarm))));
        }
    }

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

/// Runs one storage operation against this vm's cell.
fn with_storage<T>(
    lua: &Lua,
    operation: impl FnOnce(&mut crate::storage::SqliteStorage) -> Result<T, String>,
) -> mlua::Result<T> {
    let cell = lua
        .app_data_ref::<crate::storage::StorageCell>()
        .ok_or_else(|| {
            mlua::Error::RuntimeError("This object has no durable storage.".to_owned())
        })?;
    let mut storage = cell.0.borrow_mut();
    operation(&mut storage).map_err(mlua::Error::RuntimeError)
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
                lua.to_value(&rows)
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
                    Some(row) => lua.to_value(&row),
                    None => Ok(mlua::Value::Nil),
                }
            },
        )?,
    )?;

    Ok(sql)
}

/// Installs the receiving side: resolve the class method in this vm and
/// run it with the object's state table first.
fn install_dispatch(lua: &Lua) -> mlua::Result<()> {
    lua.globals().set(
        "__dispatch",
        lua.create_async_function(|lua, payload: mlua::Value| async move {
            let call: DispatchCall = lua.from_value(payload)?;

            // The method's own outbound calls extend this stack; installing
            // it per dispatch is safe because this vm runs one call at a
            // time by construction. The class rides along for `set_alarm`,
            // which needs to know whose `alarm` method to schedule.
            lua.set_app_data(CallChain(call.chain.clone()));
            lua.set_app_data(CurrentDispatch {
                class: call.class.clone(),
            });

            let classes: Table = lua.named_registry_value(CLASSES_KEY)?;
            let class: Table = classes.get(call.class.as_str()).map_err(|_| {
                mlua::Error::RuntimeError(format!("No object class '{}'.", call.class))
            })?;
            let method: mlua::Function = class.get(call.method.as_str()).map_err(|_| {
                mlua::Error::RuntimeError(format!(
                    "Object class '{}' has no method '{}'.",
                    call.class, call.method
                ))
            })?;

            // The state table is the object's identity surface: plain keys
            // are in-memory and live as long as the pinned vm; `state.sql`
            // is the durable half, present when the host opened storage.
            let (state, state_is_new) = match lua.named_registry_value::<Table>(STATE_KEY) {
                Ok(state) => (state, false),
                Err(_) => {
                    let state = lua.create_table()?;
                    if lua.app_data_ref::<crate::storage::StorageCell>().is_some() {
                        state.set("sql", sql_surface(&lua)?)?;
                    }
                    state.set("now", lua.create_function(|_, ()| Ok(unix_now_ms()))?)?;
                    state.set("set_alarm", lua.create_function(set_alarm)?)?;
                    lua.set_named_registry_value(STATE_KEY, state.clone())?;
                    (state, true)
                }
            };

            // `init` runs exactly once per object: for stored objects the
            // file is the record (a failed init retries next call); without
            // storage, once per vm life, which is when state is fresh too.
            if state_is_new && call.method != "init" {
                let fresh = match lua.app_data_ref::<crate::storage::StorageCell>() {
                    Some(cell) => {
                        let fresh = cell.0.borrow_mut().is_fresh();
                        fresh.map_err(mlua::Error::RuntimeError)?
                    }
                    None => true,
                };

                if fresh && let Ok(init) = class.get::<mlua::Function>("init") {
                    init.call_async::<()>(state.clone()).await?;
                    if let Some(cell) = lua.app_data_ref::<crate::storage::StorageCell>() {
                        cell.0
                            .borrow_mut()
                            .mark_initialized()
                            .map_err(mlua::Error::RuntimeError)?;
                    }
                }
            }

            let mut multi = mlua::MultiValue::new();
            multi.push_back(mlua::Value::Table(state));
            for argument in call.args {
                multi.push_back(lua.to_value(&argument)?);
            }

            method.call_async::<mlua::Value>(multi).await
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_the_way_scripts_write_them() {
        assert_eq!(parse_duration_ms("500ms").unwrap(), 500);
        assert_eq!(parse_duration_ms("30s").unwrap(), 30_000);
        assert_eq!(parse_duration_ms("10m").unwrap(), 600_000);
        assert_eq!(parse_duration_ms("24h").unwrap(), 86_400_000);
        assert_eq!(parse_duration_ms("7d").unwrap(), 604_800_000);
        assert_eq!(parse_duration_ms("1.5s").unwrap(), 1500);
        // A bare number is seconds.
        assert_eq!(parse_duration_ms("2").unwrap(), 2000);

        assert!(parse_duration_ms("soon").is_err());
        assert!(parse_duration_ms("10 fortnights").is_err());
    }
}
