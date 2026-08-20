//! Platform classes as rust primitives.
//!
//! A method call targeting a `__`-prefixed class the platform implements
//! never enters the vm: [`dispatch`] decodes the same payload the Lua
//! `__dispatch` speaks and routes it to the class's module. The vm is
//! entered in exactly one place, [`fire_listener`], when a platform class
//! must run user code (a queue delivery, a cron fire).
//!
//! Everything here rides the object substrate unchanged: the mailbox
//! serializes calls, the dispatch guard owns the transaction, alarms and
//! shipping work exactly as they do for user classes. Only the method
//! bodies are native.

pub mod cron;
pub mod database;
pub mod queue;

use mlua::LuaSerdeExt;
use serde::Deserialize;

use crate::extensions::objects::{AlarmCell, CallChain, PendingAlarm, unix_now_ms};
use crate::runtime::ActiasRuntime;

/// One platform method call, the same shape the Lua `__dispatch` decodes.
#[derive(Deserialize)]
pub(crate) struct Call {
    pub class: String,
    pub method: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub args: Vec<serde_json::Value>,
    /// The stack this call rides on, own key included; installed so a
    /// fired listener's outbound object calls extend it.
    #[serde(default)]
    pub chain: Vec<String>,
}

/// Whether `payload` targets a platform class; the `__` prefix is
/// reserved at declaration time, so everything carrying it is ours and
/// user classes fall through to `__dispatch`.
pub(crate) fn handles(method: &str, payload: &serde_json::Value) -> bool {
    method == "__dispatch"
        && payload["class"]
            .as_str()
            .is_some_and(|class| class.starts_with("__"))
}

/// Runs one platform method call against this vm's storage and alarm
/// cells.
///
/// # Errors
/// Returns [`crate::objects::ObjectError::Call`] with the same user-safe
/// texts a Lua-bodied method would produce.
pub(crate) async fn dispatch(
    runtime: &ActiasRuntime,
    payload: serde_json::Value,
) -> Result<serde_json::Value, crate::objects::ObjectError> {
    let call: Call = serde_json::from_value(payload)
        .map_err(|e| crate::objects::ObjectError::Call(format!("Malformed object call: {e}")))?;

    // One call at a time by construction, so installing per dispatch is
    // safe; the Lua dispatch does the same for user classes.
    runtime.set_app_data(CallChain(call.chain.clone()));

    let result = match call.class.as_str() {
        crate::extensions::objects::QUEUE_CLASS => queue::dispatch(runtime, &call).await,
        crate::extensions::objects::CRON_CLASS => cron::dispatch(runtime, &call).await,
        crate::extensions::objects::DATABASE_CLASS => database::dispatch(runtime, &call).await,
        other => Err(format!("No object class '{other}'.")),
    };

    result.map_err(crate::objects::ObjectError::Call)
}

/// Runs one operation against this vm's storage cell; the borrow never
/// outlives the closure, so callers are free to await between operations.
pub(crate) fn with_storage<T>(
    runtime: &ActiasRuntime,
    operation: impl FnOnce(&mut crate::storage::SqliteStorage) -> Result<T, String>,
) -> Result<T, String> {
    let cell = runtime
        .app_data_ref::<crate::storage::StorageCell>()
        .ok_or_else(|| "This object has no durable storage.".to_owned())?;
    let mut storage = cell.0.borrow_mut();
    operation(&mut storage)
}

/// Fires the script's listener for `event` and reports the delivery
/// verdict. Errors are contained here rather than by a Lua pcall: the
/// handler may yield (async platform calls), and Luau cannot yield across
/// a pcall's C boundary. A failing handler is logged and never unwinds
/// into the platform method that fired it.
pub(crate) async fn fire_listener(
    runtime: &ActiasRuntime,
    event: &str,
    payload: &serde_json::Value,
) -> bool {
    let Ok(listener) = runtime.listener(event) else {
        actias_common::tracing::warn!(event, "no listener registered for event");
        return false;
    };
    let argument = match runtime.to_value(payload) {
        Ok(argument) => argument,
        Err(error) => {
            actias_common::tracing::warn!(%error, event, "event payload did not convert");
            return false;
        }
    };
    if let Err(error) = listener.call_async::<mlua::Value>(argument).await {
        actias_common::tracing::warn!(%error, event, "event handler failed");
        return false;
    }
    true
}

/// Arms this object's one alarm `delay_ms` from now; setting replaces.
/// Writes both homes the same way the Lua `set_alarm` does: the persisted
/// row rides the call's transaction, the in-memory cell wakes the task
/// loop.
///
/// # Errors
/// Returns SQLite's message when the persisted row cannot be written.
pub(crate) fn set_alarm(
    runtime: &ActiasRuntime,
    class: &str,
    name: &str,
    delay_ms: i64,
) -> Result<(), String> {
    let own_key = runtime
        .app_data_ref::<CallChain>()
        .and_then(|chain| chain.0.last().cloned())
        .unwrap_or_default();

    let alarm = PendingAlarm {
        due_ms: unix_now_ms() + delay_ms.max(0),
        class: class.to_owned(),
        name: name.to_owned(),
        own_key,
    };

    with_storage(runtime, |storage| {
        storage.save_alarm(alarm.due_ms, &alarm.class, &alarm.name, &alarm.own_key)
    })?;

    match runtime.app_data_ref::<AlarmCell>() {
        Some(cell) => *cell.0.borrow_mut() = Some(alarm),
        None => {
            runtime.set_app_data(AlarmCell(std::cell::RefCell::new(Some(alarm))));
        }
    }

    Ok(())
}
