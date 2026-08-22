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

/// The built-in class names live in [`actias_common::classes`], because
/// object identities cross service boundaries; re-exported here where the
/// runtime consumes them. The router special-cases [`DATABASE_CLASS`]'s
/// read methods for the mailbox bypass.
pub use actias_common::classes::{CRON_CLASS, DATABASE_CLASS, QUEUE_CLASS, WORKFLOW_CLASS};

/// Milliseconds until a cron event's next occurrence. The expression is
/// whatever follows `cron:`; classic five-field expressions gain a seconds
/// column, since the parser wants six.
pub fn cron_delay_ms(event: &str) -> Result<i64, String> {
    use std::str::FromStr;

    let expr = event.strip_prefix("cron:").unwrap_or(event).trim();
    let normalized = if expr.split_whitespace().count() == 5 {
        format!("0 {expr}")
    } else {
        expr.to_owned()
    };

    let schedule = cron::Schedule::from_str(&normalized)
        .map_err(|e| format!("'{expr}' is not a cron expression: {e}"))?;
    let next = schedule
        .upcoming(chrono::Utc)
        .next()
        .ok_or_else(|| format!("'{expr}' never occurs"))?;

    Ok((next.timestamp_millis() - unix_now_ms()).max(1000))
}

/// Registry key of the object's state table; exists only in pinned vms,
/// created on their first dispatched call.
const STATE_KEY: &str = "object_state";

/// The calling script's identity, when the routing layer knows it; the
/// queue journal records it as a message's producer.
#[derive(Clone)]
pub struct CallerIdentity {
    /// Public identifier, the name a human recognizes.
    pub script: String,
    /// Revision id the caller executed as.
    pub revision: String,
}

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
    /// Filled by the routing layer, which knows whose vm the call left;
    /// [`None`] for calls with no script behind them (dashboard reads).
    pub caller: Option<CallerIdentity>,
}

/// The call chain the currently dispatched method arrived on; app data in
/// pinned vms, absent in request vms. One slot suffices because a pinned
/// vm runs exactly one call at a time.
pub struct CallChain(pub Vec<String>);

#[derive(Clone)]
pub struct PendingAlarm {
    /// Unix milliseconds the alarm is due at.
    pub due_ms: i64,
    /// Class whose `alarm` method runs.
    pub class: String,
    /// Instance name, so the alarm dispatch identifies its object the way
    /// every other dispatch does.
    pub name: String,
    /// The object's own key, seeding the alarm dispatch's call chain.
    pub own_key: String,
}

/// Milliseconds since the unix epoch, the clock `state.now()` exposes and
/// the alarm loop schedules against.
/// The virtual-clock offset `actias test` advances; zero everywhere
/// else, so production time is wall time.
static TEST_CLOCK_OFFSET_MS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// Fast-forwards every platform clock in this process: test harness
/// machinery, which is why a 24h await times out in a millisecond test.
pub fn advance_clock_for_tests(ms: i64) {
    TEST_CLOCK_OFFSET_MS.fetch_add(ms.max(0), std::sync::atomic::Ordering::Relaxed);
}

pub fn unix_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
        + TEST_CLOCK_OFFSET_MS.load(std::sync::atomic::Ordering::Relaxed)
}

/// A duration written the way scripts write them: "500ms", "30s", "10m",
/// "24h", "7d", or a bare number of seconds.
pub fn parse_duration_ms(raw: &str) -> Result<i64, String> {
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
    /// The instance name; databases use it to find their migration files.
    #[serde(default)]
    name: String,
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
        // User classes only; platform classes dispatch in rust and never
        // appear here.
        let classes = lua.create_table()?;
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

        // `queue "name"`: a durable message queue, sugar over an object of
        // the built-in class. `:send` enqueues; the revision declaring
        // `on "queue:<name>"` consumes, driven by the object's alarm.
        lua.globals().set(
            "queue",
            lua.create_function(|lua, name: String| {
                ActiasRuntime::assert_declaration_phase(lua, "queue")?;
                ActiasRuntime::assert_contract_allows(lua, ContractKind::Queue, &name)?;
                ActiasRuntime::record_queue_declaration(lua, &name);
                instance_handle(lua, QUEUE_CLASS.to_owned(), name)
            })?,
        )?;

        // `workflows "name"`: the definition handle, same one the
        // declaration returns; cross-script callers start and address
        // runs through it.
        lua.globals().set(
            "workflows",
            lua.create_function(|lua, name: String| {
                ActiasRuntime::assert_declaration_phase(lua, "workflows")?;
                workflow_definition_handle(lua, name)
            })?,
        )?;

        // `objects "Class"`: reference a class declared elsewhere. It mints
        // the same handle; whether the class exists is the callee's truth.
        // Platform classes are not addressable this way.
        lua.globals().set(
            "objects",
            lua.create_function(|lua, class: String| {
                ActiasRuntime::assert_declaration_phase(lua, "objects")?;
                if class.starts_with("__") {
                    return Err(mlua::Error::RuntimeError(format!(
                        "Class name '{class}' is reserved for the platform."
                    )));
                }
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
                // The publishes key is contract data; record it exactly
                // as the publish-time extraction spells it.
                if let Ok(publishes) = methods.get::<Table>("publishes") {
                    let mut index = 1;
                    while let Ok(entry) = publishes.get::<mlua::Value>(index) {
                        if entry.is_nil() {
                            break;
                        }
                        if let Some(topic) = entry.as_string().and_then(|s| s.to_str().ok()) {
                            ActiasRuntime::record_publishes_declaration(
                                lua,
                                format!("{class}:{}", &*topic),
                            );
                        }
                        index += 1;
                    }
                    for pair in publishes.pairs::<mlua::Value, mlua::Value>() {
                        let Ok((key, value)) = pair else { continue };
                        let (Some(topic), Some(policy)) = (
                            key.as_string()
                                .and_then(|s| s.to_str().ok().map(|s| s.to_string())),
                            value
                                .as_string()
                                .and_then(|s| s.to_str().ok().map(|s| s.to_string())),
                        ) else {
                            continue;
                        };
                        ActiasRuntime::record_publishes_declaration(
                            lua,
                            format!("{class}:{topic}={policy}"),
                        );
                    }
                }
                let classes: Table = lua.named_registry_value(CLASSES_KEY)?;
                classes.set(class.as_str(), methods)?;
                class_handle(lua, class.clone())
            })
        })?;

        Ok(mlua::Value::Function(declaration))
    }
}

/// Names only the platform may invoke on an object: the hooks (called
/// as `__`-prefixed internal methods) and their public spellings, which
/// handles refuse outright so a `__`-method arriving at `__dispatch` is
/// provably platform-originated.
pub const RESERVED_METHODS: [&str; 5] = ["init", "alarm", "receive", "follow", "hooks"];

/// Whether a method name may travel through a handle.
fn callable_method(method: &str) -> bool {
    !method.starts_with('_') && !RESERVED_METHODS.contains(&method)
}

/// Resolves a platform hook on a class: the `hooks` table is the home;
/// flat `init`/`alarm` remain the deprecated long form.
fn resolve_hook(class: &Table, name: &str) -> Option<mlua::Function> {
    if let Ok(hooks) = class.get::<Table>("hooks")
        && let Ok(function) = hooks.get::<mlua::Function>(name)
    {
        return Some(function);
    }
    if matches!(name, "init" | "alarm") {
        return class.get::<mlua::Function>(name).ok();
    }
    None
}

/// The identity value gates and `receive` see: name, transport, a
/// canonical id, and `:is(Class)` against a class VALUE, never a string.
fn make_identity(lua: &Lua, class: &str, name: &str, transport: &str) -> mlua::Result<Table> {
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

/// The class handle: `Class(name)` (or the long `Class:get(name)`) mints
/// an instance handle.
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

    // Callable: `Channel(id)` is the blessed spelling of `:get(id)`.
    let meta = lua.create_table()?;
    meta.set(
        "__call",
        lua.create_function(|lua, (this, name): (Table, String)| {
            let class: String = this.get("__class")?;
            instance_handle(lua, class, name)
        })?,
    )?;
    handle.set_metatable(Some(meta))?;

    Ok(handle)
}

/// The workflow definition handle `workflow "name" (fn)` returns and
/// `workflows "name"` looks up: `start` mints a run and kicks its first
/// attempt, `get` addresses an existing run. Run handles are ordinary
/// instance handles on the workflow class, so signal/cancel/status route
/// like any object method.
pub(crate) fn workflow_definition_handle(lua: &Lua, definition: String) -> mlua::Result<Table> {
    let handle = lua.create_table()?;
    handle.set("__definition", definition)?;

    let meta = lua.create_table()?;
    meta.set(
        "__index",
        lua.create_function(|lua, (this, method): (Table, String)| {
            let definition: String = this.get("__definition")?;
            match method.as_str() {
                "start" => lua.create_async_function(move |lua, args: mlua::MultiValue| {
                    let definition = definition.clone();
                    async move {
                        let mut values = args.into_iter();
                        let _receiver = values.next();
                        let input = values
                            .next()
                            .map(|value| lua.from_value::<serde_json::Value>(value))
                            .transpose()?
                            .unwrap_or(serde_json::Value::Null);
                        let id = values
                            .next()
                            .and_then(|value| match value {
                                mlua::Value::Table(opts) => {
                                    opts.get::<Option<String>>("id").ok().flatten()
                                }
                                _ => None,
                            })
                            .ok_or_else(|| {
                                mlua::Error::RuntimeError(
                                    "start takes the input and { id = \"...\" }; the id is \
                                     the run's identity, so a retried start joins it."
                                        .to_owned(),
                                )
                            })?;
                        if id.trim().is_empty() || id.contains('/') {
                            return Err(mlua::Error::RuntimeError(
                                "A run id is a non-empty string without '/'.".to_owned(),
                            ));
                        }

                        let name = format!("{definition}/{id}");
                        let run = instance_handle(&lua, WORKFLOW_CLASS.to_owned(), name.clone())?;
                        let start: mlua::Function = run.get("start")?;
                        let outcome: mlua::Value = start
                            .call_async((run.clone(), lua.to_value(&input)?))
                            .await?;
                        run.set("started", outcome)?;
                        Ok(run)
                    }
                }),
                "get" => lua.create_function(move |lua, (_this, id): (Table, String)| {
                    instance_handle(lua, WORKFLOW_CLASS.to_owned(), format!("{definition}/{id}"))
                }),
                other => Err(mlua::Error::RuntimeError(format!(
                    "A workflow definition handle has start and get, not '{other}'."
                ))),
            }
        })?,
    )?;
    handle.set_metatable(Some(meta))?;
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
            if !callable_method(&method) {
                return Err(mlua::Error::RuntimeError(format!(
                    "'{method}' is a platform hook; only the platform calls it."
                )));
            }

            lua.create_async_function(move |lua, args: mlua::MultiValue| {
                let class = class.clone();
                let name = name.clone();
                let method = method.clone();

                async move {
                    // In workflow vms, effects live inside steps alone;
                    // everywhere else this is a no-op.
                    crate::platform::workflow::assert_effects_allowed(&lua)?;
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
                        // The router knows whose vm this is; it fills this.
                        caller: None,
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

/// Which class and instance the pinned vm is currently dispatching for.
struct CurrentDispatch {
    class: String,
    name: String,
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

/// How a topic may be followed, read from the class's `publishes` key:
/// array entries gate through `hooks.follow`, keyed entries carry a
/// built-in policy, anything else is not published at all.
enum TopicPolicy {
    Absent,
    Hooked,
    SelfOnly,
}

fn topic_policy(class: &Table, topic: &str) -> TopicPolicy {
    let Ok(publishes) = class.get::<Table>("publishes") else {
        return TopicPolicy::Absent;
    };
    if let Ok(policy) = publishes.get::<String>(topic) {
        if policy == "self" {
            return TopicPolicy::SelfOnly;
        }
        return TopicPolicy::Absent;
    }
    let mut index = 1;
    while let Ok(entry) = publishes.get::<mlua::Value>(index) {
        if entry.is_nil() {
            break;
        }
        if entry
            .as_string()
            .and_then(|s| s.to_str().ok())
            .map(|s| *s == *topic)
            .unwrap_or(false)
        {
            return TopicPolicy::Hooked;
        }
        index += 1;
    }
    TopicPolicy::Absent
}

/// Builds (or reuses) the object's state table: plain keys are in-memory
/// and live as long as the pinned vm; `state.sql` is the durable half;
/// the stream verbs ride here beside `set_alarm`.
fn object_state(lua: &Lua) -> mlua::Result<(Table, bool)> {
    if let Ok(state) = lua.named_registry_value::<Table>(STATE_KEY) {
        return Ok((state, false));
    }
    let state = lua.create_table()?;
    let stored = lua
        .app_data_ref::<Arc<crate::objects::ObjectHome>>()
        .is_some_and(|home| home.has_storage());
    if stored {
        state.set("sql", sql_surface(lua)?)?;
    }
    state.set("now", lua.create_function(|_, ()| Ok(unix_now_ms()))?)?;
    state.set("set_alarm", lua.create_function(set_alarm)?)?;

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
            let classes: Table = lua.named_registry_value(CLASSES_KEY)?;
            let class: Table = classes.get(current.0.as_str())?;
            if matches!(topic_policy(&class, &topic), TopicPolicy::Absent) {
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
            }),
        ],
        chain,
        caller: None,
    })
    .await
    .map_err(mlua::Error::RuntimeError)?;
    Ok(())
}

/// Runs `init` exactly once per object before any other work touches
/// state; the file is the record for stored objects.
async fn run_init_if_fresh(
    lua: &Lua,
    class: &Table,
    state: &Table,
    state_is_new: bool,
    method: &str,
) -> mlua::Result<()> {
    if method == "init" || method == "__init" {
        return Ok(());
    }
    let home = lua
        .app_data_ref::<Arc<crate::objects::ObjectHome>>()
        .map(|home| home.clone())
        .filter(|home| home.has_storage());
    // Stored objects consult the file every call: a failed first call
    // rolls init back WITH the mark (user_version is transactional), so
    // the next call retries it; vm memory must not veto that. Without
    // storage, once per vm life is the record.
    let fresh = match &home {
        Some(home) => home
            .with_storage(|storage| storage.is_fresh())
            .map_err(mlua::Error::RuntimeError)?,
        None => state_is_new,
    };
    if fresh && let Some(init) = resolve_hook(class, "init") {
        init.call_async::<()>(state.clone()).await?;
        if let Some(home) = &home {
            home.with_storage(|storage| storage.mark_initialized())
                .map_err(mlua::Error::RuntimeError)?;
        }
    }
    Ok(())
}

/// The internal platform verbs, `__`-prefixed and therefore
/// platform-originated (handles refuse the spelling): the alarm hook,
/// the follow gate, unfollow, and receive delivery.
async fn dispatch_internal(
    lua: &Lua,
    class: &Table,
    call: &DispatchCall,
    verb: &str,
) -> mlua::Result<mlua::Value> {
    let (state, state_is_new) = object_state(lua)?;
    state.set("name", call.name.as_str())?;
    run_init_if_fresh(lua, class, &state, state_is_new, &call.method).await?;

    match verb {
        "alarm" => {
            let Some(alarm) = resolve_hook(class, "alarm") else {
                return Err(mlua::Error::RuntimeError(format!(
                    "Object class '{}' has no alarm hook.",
                    call.class
                )));
            };
            alarm.call_async::<mlua::Value>(state).await
        }
        "follow" | "unfollow" => {
            let topic = call
                .args
                .first()
                .and_then(|value| value.as_str())
                .ok_or_else(|| mlua::Error::RuntimeError("follow carries a topic.".to_owned()))?
                .to_owned();
            let filter = call.args.get(1).cloned().unwrap_or(serde_json::Value::Null);
            let follower = call.args.get(2).cloned().unwrap_or(serde_json::Value::Null);
            let follower_class = follower["class"].as_str().unwrap_or_default().to_owned();
            let follower_name = follower["name"].as_str().unwrap_or_default().to_owned();
            let transport = follower["transport"]
                .as_str()
                .unwrap_or("object")
                .to_owned();
            let home = lua
                .app_data_ref::<Arc<crate::objects::ObjectHome>>()
                .map(|home| home.clone())
                .filter(|home| home.has_storage())
                .ok_or_else(|| {
                    mlua::Error::RuntimeError("streams need object storage.".to_owned())
                })?;

            if verb == "unfollow" {
                home.with_storage(|storage| {
                    crate::streams::delete_edge(storage, &follower_class, &follower_name, &topic)
                })
                .map_err(mlua::Error::RuntimeError)?;
                return Ok(mlua::Value::Boolean(true));
            }

            let accepted = match topic_policy(class, &topic) {
                TopicPolicy::Absent => {
                    return Err(mlua::Error::RuntimeError(format!(
                        "'{}' does not publish '{topic}'.",
                        call.class
                    )));
                }
                TopicPolicy::SelfOnly => follower_class == call.class && follower_name == call.name,
                TopicPolicy::Hooked => {
                    let Some(gate) = resolve_hook(class, "follow") else {
                        return Err(mlua::Error::RuntimeError(format!(
                            "'{}' has no follow gate; nobody may follow '{topic}'.",
                            call.class
                        )));
                    };
                    let identity = make_identity(lua, &follower_class, &follower_name, &transport)?;
                    gate.call_async::<mlua::Value>((state, topic.clone(), identity))
                        .await?
                        .as_boolean()
                        .unwrap_or(false)
                }
            };
            if !accepted {
                return Err(mlua::Error::RuntimeError(format!(
                    "follow refused: '{topic}' on {}/{}.",
                    call.class, call.name
                )));
            }
            let kind = if transport == "connection" {
                "connection"
            } else {
                "object"
            };
            let filter_option = if filter.is_null() {
                None
            } else {
                Some(&filter)
            };
            home.with_storage(|storage| {
                crate::streams::upsert_edge(
                    storage,
                    kind,
                    &follower_class,
                    &follower_name,
                    &topic,
                    filter_option,
                )
            })
            .map_err(mlua::Error::RuntimeError)?;
            Ok(mlua::Value::Boolean(true))
        }
        "receive" => {
            let Some(receive) = resolve_hook(class, "receive") else {
                // No receive hook: delivered and discarded, by design.
                return Ok(mlua::Value::Nil);
            };
            let event_json = call
                .args
                .first()
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let event = lua.create_table()?;
            event.set("topic", event_json["topic"].as_str().unwrap_or_default())?;
            event.set("data", lua.to_value(&event_json["data"])?)?;
            let from = make_identity(
                lua,
                event_json["from"]["class"].as_str().unwrap_or_default(),
                event_json["from"]["name"].as_str().unwrap_or_default(),
                "object",
            )?;
            event.set("from", from)?;
            receive.call_async::<mlua::Value>((state, event)).await
        }
        other => Err(mlua::Error::RuntimeError(format!(
            "No platform verb '__{other}'."
        ))),
    }
}

/// Installs the receiving side: resolve the class method in this vm and
/// run it with the object's state table first.
fn install_dispatch(lua: &Lua) -> mlua::Result<()> {
    lua.globals().set(
        "__dispatch",
        lua.create_async_function(|lua, payload: mlua::Value| async move {
            let call: DispatchCall = lua.from_value(payload)?;

            // Platform classes dispatch in rust before the vm is entered;
            // one arriving here is a routing bug, refused like any other
            // unknown class.
            if call.class.starts_with("__") {
                return Err(mlua::Error::RuntimeError(format!(
                    "No object class '{}'.",
                    call.class
                )));
            }

            // The method's own outbound calls extend this stack; installing
            // it per dispatch is safe because this vm runs one call at a
            // time by construction. The class rides along for `set_alarm`,
            // which needs to know whose `alarm` method to schedule.
            lua.set_app_data(CallChain(call.chain.clone()));
            lua.set_app_data(CurrentDispatch {
                class: call.class.clone(),
                name: call.name.clone(),
            });

            let classes: Table = lua.named_registry_value(CLASSES_KEY)?;
            let class: Table = classes.get(call.class.as_str()).map_err(|_| {
                mlua::Error::RuntimeError(format!("No object class '{}'.", call.class))
            })?;

            // Internal platform verbs arrive `__`-prefixed; handles refuse
            // that spelling, so these are platform-originated.
            if let Some(verb) = call.method.strip_prefix("__") {
                let result = dispatch_internal(&lua, &class, &call, verb).await?;
                return Ok(result);
            }
            if !callable_method(&call.method) {
                return Err(mlua::Error::RuntimeError(format!(
                    "'{}' is a platform hook; only the platform calls it.",
                    call.method
                )));
            }

            let method: mlua::Function = class.get(call.method.as_str()).map_err(|_| {
                mlua::Error::RuntimeError(format!(
                    "Object class '{}' has no method '{}'.",
                    call.class, call.method
                ))
            })?;

            let (state, state_is_new) = object_state(&lua)?;

            // The object knows its own name; the queue class keys its
            // event off it, and user code gets it for free.
            state.set("name", call.name.as_str())?;

            run_init_if_fresh(&lua, &class, &state, state_is_new, &call.method).await?;

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
