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
pub mod workflow;

use mlua::LuaSerdeExt;
use serde::Deserialize;

use crate::extensions::objects::{CallChain, PendingAlarm, unix_now_ms};
use crate::objects::ObjectHome;
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
    /// Who is calling, when the router knows: the queue journal records
    /// it as the producer. Platform-internal dispatches (alarms) have
    /// none.
    #[serde(default)]
    pub caller: Option<Caller>,
}

/// The calling script's identity, as the router sees it.
#[derive(Deserialize, Clone)]
pub(crate) struct Caller {
    /// Public identifier, the name a human recognizes.
    pub script: String,
    /// Revision id the caller executed as.
    pub revision: String,
}

/// What a platform method may touch, passed explicitly: the object's own
/// state plus its call identity. Nothing in here is guest-shaped; the
/// guest runtime appears only where a listener must actually fire.
pub(crate) struct PlatformContext<'a> {
    pub home: &'a ObjectHome,
    /// The instance name.
    pub name: &'a str,
    /// The object's own key, seeding any alarm it arms.
    pub own_key: &'a str,
}

/// One typed dashboard read against an object's file. The transport layer
/// (the worker's internal endpoint today, the WorkerData service later)
/// only maps its parameters onto a variant and picks which file answers
/// (local, replica); everything from file to structured value, including
/// what a file that predates the schema contains, is this module's
/// business.
pub enum PlatformRead {
    /// Queue depth, in flight, oldest pending and dead letters.
    QueueStats,
    /// Queue journal rows after `since`, oldest first.
    QueueEvents { since: i64 },
    /// The queue's live and dead message rows with display states.
    QueueMessages,
    /// A database's file size and user tables with their shapes; any
    /// object's storage answers it, user classes included.
    DatabaseOverview,
    /// One read-only SQL statement against the file, under the same
    /// authorizer script SQL runs with; how the console browses an
    /// object's storage without dispatching into its vm.
    Query { sql: String },
    /// A workflow run's derived status plus journal head facts.
    WorkflowStatus,
    /// The workflow journal after `since`, oldest first: what the CI
    /// view folds.
    WorkflowJournal { since: i64 },
}

impl PlatformRead {
    /// The overview read a dashboard asks for by class name; [`None`] for
    /// platform classes without one. A user class's storage is a SQLite
    /// file like any database, so it answers the overview too.
    pub fn stats_for_class(class: &str) -> Option<Self> {
        match class {
            crate::extensions::objects::QUEUE_CLASS => Some(Self::QueueStats),
            actias_common::classes::WORKFLOW_CLASS => Some(Self::WorkflowStatus),
            crate::extensions::objects::DATABASE_CLASS => Some(Self::DatabaseOverview),
            class if class.starts_with("__") => None,
            _ => Some(Self::DatabaseOverview),
        }
    }

    /// Runs the read against a file, opened read-only. Blocking SQLite io;
    /// async callers wrap it in `spawn_blocking`.
    ///
    /// # Errors
    /// Returns the user-safe text of whatever failed, like a dispatched
    /// method would.
    pub fn run(&self, file: &std::path::Path) -> Result<serde_json::Value, String> {
        let mut storage = crate::storage::SqliteStorage::open_read_only(file)?;
        let value = match self {
            Self::QueueStats => serde_json::to_value(queue::read_stats(&mut storage)?),
            Self::QueueEvents { since } => {
                serde_json::to_value(queue::read_events(&mut storage, *since)?)
            }
            Self::QueueMessages => serde_json::to_value(queue::read_messages(&mut storage)?),
            Self::DatabaseOverview => serde_json::to_value(database::read_overview(&mut storage)?),
            Self::Query { sql } => serde_json::to_value(storage.query(sql, &[])?),
            Self::WorkflowStatus => {
                let entries = workflow::read_journal_readonly(&mut storage)?;
                let status = workflow::run_status(&entries);
                let input = entries
                    .first()
                    .filter(|e| e.kind == workflow::EntryKind::Started)
                    .map(|e| e.data["input"].clone())
                    .unwrap_or(serde_json::Value::Null);
                return Ok(serde_json::json!({
                    "status": status,
                    "at": workflow::at_step(&entries),
                    "input": input,
                    "entries": entries.len(),
                    "started_at": entries.first().map(|e| e.at),
                    "updated_at": entries.last().map(|e| e.at),
                }));
            }
            Self::WorkflowJournal { since } => {
                serde_json::to_value(workflow::read_journal_readonly_from(&mut storage, *since)?)
            }
        };
        value.map_err(|e| e.to_string())
    }
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
    home: &ObjectHome,
    payload: serde_json::Value,
) -> Result<serde_json::Value, crate::objects::ObjectError> {
    let call: Call = serde_json::from_value(payload)
        .map_err(|e| crate::objects::ObjectError::Call(format!("Malformed object call: {e}")))?;

    // One call at a time by construction, so installing per dispatch is
    // safe; the Lua dispatch does the same for user classes. Only a fired
    // listener's outbound calls read this.
    runtime.set_app_data(CallChain(call.chain.clone()));

    let context = PlatformContext {
        home,
        name: &call.name,
        own_key: call.chain.last().map(String::as_str).unwrap_or_default(),
    };

    let result = match call.class.as_str() {
        crate::extensions::objects::QUEUE_CLASS => queue::dispatch(runtime, &context, &call).await,
        crate::extensions::objects::CRON_CLASS => cron::dispatch(runtime, &context, &call).await,
        crate::extensions::objects::DATABASE_CLASS => database::dispatch(&context, &call),
        actias_common::classes::WORKFLOW_CLASS => {
            workflow::dispatch(runtime, &context, &call).await
        }
        other => Err(format!("No object class '{other}'.")),
    };

    result.map_err(crate::objects::ObjectError::Call)
}

/// Fires the script's listener for `event` and reports the delivery
/// verdict: [`Ok`] on success, the user-safe failure text otherwise (the
/// queue journals it per attempt). Errors are contained here rather than
/// by a Lua pcall: the handler may yield (async platform calls), and Luau
/// cannot yield across a pcall's C boundary. A failing handler is logged
/// and never unwinds into the platform method that fired it.
pub(crate) async fn fire_listener(
    runtime: &ActiasRuntime,
    event: &str,
    payload: &serde_json::Value,
) -> Result<(), String> {
    let Ok(listener) = runtime.listener(event) else {
        actias_common::tracing::warn!(event, "no listener registered for event");
        return Err(format!("no listener registered for '{event}'"));
    };
    let argument = match runtime.to_value(payload) {
        Ok(argument) => argument,
        Err(error) => {
            actias_common::tracing::warn!(%error, event, "event payload did not convert");
            return Err(format!("event payload did not convert: {error}"));
        }
    };
    if let Err(error) = listener.call_async::<mlua::Value>(argument).await {
        actias_common::tracing::warn!(%error, event, "event handler failed");
        return Err(error.to_string());
    }
    Ok(())
}

/// Arms this object's one alarm `delay_ms` from now; setting replaces.
/// Writes both homes the same way the Lua `set_alarm` does: the persisted
/// row rides the call's transaction, the in-memory cell wakes the task
/// loop.
///
/// # Errors
/// Returns SQLite's message when the persisted row cannot be written.
pub(crate) fn set_alarm(
    context: &PlatformContext<'_>,
    class: &str,
    delay_ms: i64,
) -> Result<(), String> {
    context.home.set_alarm(PendingAlarm {
        due_ms: unix_now_ms() + delay_ms.max(0),
        class: class.to_owned(),
        name: context.name.to_owned(),
        own_key: context.own_key.to_owned(),
    })
}
