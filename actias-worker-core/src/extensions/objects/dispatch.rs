//! The dispatch protocol: what a routed call looks like, the chain and
//! gates it must pass, hook resolution, init-once, and `__dispatch`
//! itself, the receiving side installed in every vm.

use std::sync::Arc;

use mlua::{Lua, LuaSerdeExt, Table};
use serde::Deserialize;

use super::registry::{ClassRegistry, TopicPolicy};
use super::state::{make_identity, object_state};

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

/// Names only the platform may invoke on an object: the hooks (called
/// as `__`-prefixed internal methods) and their public spellings, which
/// handles refuse outright so a `__`-method arriving at `__dispatch` is
/// provably platform-originated.
pub const RESERVED_METHODS: [&str; 6] = ["init", "alarm", "receive", "receives", "follow", "hooks"];

/// Whether a method name may travel through a handle.
pub(super) fn callable_method(method: &str) -> bool {
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

/// Which class and instance the pinned vm is currently dispatching for.
pub(super) struct CurrentDispatch {
    pub(super) class: String,
    pub(super) name: String,
}

/// Runs `init` exactly once per object before any other work touches
/// state; the file is the record for stored objects.
async fn run_init_if_fresh(
    lua: &Lua,
    class_name: &str,
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
    // Schema from files runs before init, so init only ever seeds.
    if let (Some(home), Some(dir)) = (
        &home,
        ClassRegistry::of(lua)
            .spec(class_name)
            .and_then(|spec| spec.migrations.clone()),
    ) {
        crate::platform::database::Database::apply_declared_migrations(home, &dir)
            .map_err(mlua::Error::RuntimeError)?;
    }
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
    run_init_if_fresh(lua, &call.class, class, &state, state_is_new, &call.method).await?;

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
                let connection_id = follower["connection"].as_str().map(str::to_owned);
                home.with_storage(|storage| {
                    crate::streams::delete_edge(
                        storage,
                        &follower_class,
                        &follower_name,
                        connection_id.as_deref(),
                        &topic,
                    )
                })
                .map_err(mlua::Error::RuntimeError)?;
                return Ok(mlua::Value::Boolean(true));
            }

            let policy = ClassRegistry::of(lua)
                .spec(&call.class)
                .map(|spec| spec.topic_policy(&topic))
                .unwrap_or(TopicPolicy::Absent);
            let accepted = match policy {
                TopicPolicy::Absent => {
                    return Err(mlua::Error::RuntimeError(format!(
                        "'{}' does not publish '{topic}'.",
                        call.class
                    )));
                }
                TopicPolicy::SelfOnly => follower_class == call.class && follower_name == call.name,
                TopicPolicy::Public => true,
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
            let connection_id = follower["connection"].as_str().map(str::to_owned);
            if kind == "connection" && connection_id.is_none() {
                return Err(mlua::Error::RuntimeError(
                    "a connection follower names its connection.".to_owned(),
                ));
            }
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
                    connection_id.as_deref(),
                    &topic,
                    filter_option,
                )
            })
            .map_err(mlua::Error::RuntimeError)?;
            Ok(mlua::Value::Boolean(true))
        }
        "receive" => {
            let event_json = call
                .args
                .first()
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let from_class = event_json["from"]["class"].as_str().unwrap_or_default();
            let topic = event_json["topic"].as_str().unwrap_or_default();
            // Routing is the key: one declared handler per consumed
            // stream, "Source:topic". No entry means delivered and
            // discarded (the checker refuses the follow that would
            // make such an edge; erroring here would only make the
            // pump retry something that can never succeed).
            let key = format!("{from_class}:{topic}");
            let handler = ClassRegistry::of(lua)
                .spec(&call.class)
                .filter(|spec| spec.receives.contains(&key))
                .and_then(|_| class.get::<Table>("receives").ok())
                .and_then(|table| table.get::<mlua::Function>(key.as_str()).ok());
            let Some(handler) = handler else {
                return Ok(mlua::Value::Nil);
            };
            let event = lua.create_table()?;
            event.set("topic", topic)?;
            event.set("data", lua.to_value(&event_json["data"])?)?;
            let from = make_identity(
                lua,
                from_class,
                event_json["from"]["name"].as_str().unwrap_or_default(),
                "object",
            )?;
            event.set("from", from)?;
            handler.call_async::<mlua::Value>((state, event)).await
        }
        other => Err(mlua::Error::RuntimeError(format!(
            "No platform verb '__{other}'."
        ))),
    }
}

/// Installs the receiving side: resolve the class method in this vm and
/// run it with the object's state table first.
pub(super) fn install_dispatch(lua: &Lua) -> mlua::Result<()> {
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

            let class = ClassRegistry::of(&lua)
                .class_table(&call.class)
                .ok_or_else(|| {
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

            run_init_if_fresh(
                &lua,
                &call.class,
                &class,
                &state,
                state_is_new,
                &call.method,
            )
            .await?;

            let mut multi = mlua::MultiValue::new();
            multi.push_back(mlua::Value::Table(state));
            for argument in call.args {
                // A json null in an argument is Lua NIL, never mlua's
                // null sentinel: the sentinel is truthy, so `limit or
                // 50` style defaulting would silently keep a userdata
                // (found live: math.min exploding on a nil parameter).
                multi.push_back(
                    lua.to_value_with(
                        &argument,
                        mlua::SerializeOptions::new()
                            .serialize_none_to_null(false)
                            .serialize_unit_to_null(false),
                    )?,
                );
            }

            method.call_async::<mlua::Value>(multi).await
        })?,
    )?;

    Ok(())
}
