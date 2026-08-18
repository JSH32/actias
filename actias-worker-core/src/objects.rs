//! The serialization primitive under durable objects: one long-lived vm
//! owned by one tokio task, every call a mailbox message answered through
//! a oneshot. The input gate is the mailbox loop itself: the next message
//! is popped only after the current handler has finished, so object code
//! never observes interleaved execution, even across await points.
//!
//! This is substrate: it knows how to own a vm and serialize calls into
//! it. What a class is, where state lives and who may call arrive in the
//! layers above.

use std::collections::HashMap;
use std::sync::Arc;

use mlua::LuaSerdeExt;
use tokio::sync::{Mutex, mpsc, oneshot};

use crate::runtime::ActiasRuntime;

/// How deep one object's mailbox goes before senders wait; backpressure,
/// never a drop policy.
const MAILBOX_DEPTH: usize = 128;

/// Why a call did not return a value.
#[derive(Debug)]
pub enum ObjectError {
    /// The method failed or does not exist; the text is the script's own
    /// error, exactly as a request handler's failure would read.
    Call(String),
    /// The object's task is gone; the caller should resolve the object
    /// again rather than retry blindly.
    Gone,
}

impl std::fmt::Display for ObjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ObjectError::Call(message) => f.write_str(message),
            ObjectError::Gone => f.write_str("the object's task is gone"),
        }
    }
}

impl std::error::Error for ObjectError {}

/// One queued method call and the channel its answer travels back on.
struct ObjectCall {
    method: String,
    payload: serde_json::Value,
    reply: oneshot::Sender<Result<serde_json::Value, ObjectError>>,
}

/// A clonable address for one object's mailbox.
#[derive(Clone)]
pub struct ObjectHandle {
    sender: mpsc::Sender<ObjectCall>,
}

impl ObjectHandle {
    /// Sends one call and waits for its result; calls from any number of
    /// tasks execute one at a time in arrival order.
    ///
    /// # Errors
    /// Returns [`ObjectError::Call`] when the method fails or is missing,
    /// [`ObjectError::Gone`] when the task no longer runs.
    pub async fn call(
        &self,
        method: &str,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, ObjectError> {
        let (reply, response) = oneshot::channel();

        self.sender
            .send(ObjectCall {
                method: method.to_owned(),
                payload,
                reply,
            })
            .await
            .map_err(|_| ObjectError::Gone)?;

        response.await.map_err(|_| ObjectError::Gone)?
    }
}

/// Moves `runtime` onto its own task forever and hands back its mailbox.
///
/// The task ends when every handle is dropped; the vm drops with it.
/// Runs after a call that wrote, before its caller hears the result:
/// the output gate. Shipping a snapshot is the intended occupant.
pub type AfterWrite =
    Arc<dyn Fn() -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Tracks the storage change counter across calls, so only calls that
/// actually wrote pay the shipping toll.
struct ShipMark(std::cell::Cell<i64>);

/// Everything configurable about one pinned task; the runtime is the
/// only required piece.
#[derive(Default)]
pub struct TaskOptions {
    /// Seconds one dispatched call may run; [`None`] leaves calls unbounded.
    pub call_budget: Option<u64>,
    /// The object's durable half; [`None`] leaves state in-memory only.
    pub storage: Option<crate::storage::SqliteStorage>,
    /// Idle time after which the task hibernates: it simply ends, state
    /// already on disk, and the host revives it on the next touch. An
    /// object holding a pending alarm stays warm until it fires.
    pub hibernate_after: Option<std::time::Duration>,
    /// The output gate: runs after any call that wrote, before its caller
    /// hears the result. Snapshot shipping lives here.
    pub after_write: Option<AfterWrite>,
}

pub fn spawn_object_task(runtime: ActiasRuntime, options: TaskOptions) -> ObjectHandle {
    use crate::extensions::objects::{AlarmCell, PendingAlarm};

    let TaskOptions {
        call_budget,
        storage,
        hibernate_after,
        after_write,
    } = options;

    let (sender, mut receiver) = mpsc::channel::<ObjectCall>(MAILBOX_DEPTH);

    runtime.set_app_data(ShipMark(std::cell::Cell::new(0)));
    if let Some(mut storage) = storage {
        // A persisted alarm re-arms the moment the object is resident
        // again; past-due fires immediately. (A cold object with a due
        // alarm still needs a touch to wake, until placement can scan.)
        let pending = storage
            .load_alarm()
            .ok()
            .flatten()
            .map(|(due_ms, class, own_key)| PendingAlarm {
                due_ms,
                class,
                own_key,
            });
        runtime.set_app_data(AlarmCell(std::cell::RefCell::new(pending)));
        runtime.set_app_data(crate::storage::StorageCell(std::cell::RefCell::new(
            storage,
        )));
    }

    tokio::spawn(async move {
        // Popping only after the previous call finished is the input gate;
        // there is deliberately no concurrency inside this loop. A due
        // alarm is just one more message source, so it serializes with
        // calls exactly like they serialize with each other.
        loop {
            let pending = runtime
                .app_data_ref::<AlarmCell>()
                .and_then(|cell| cell.0.borrow().clone());

            let call = if let Some(alarm) = pending {
                // A pending alarm keeps the vm warm: hibernating past it
                // would silently drop work the object asked itself for.
                let wait = (alarm.due_ms - crate::extensions::objects::unix_now_ms()).max(0);
                tokio::select! {
                    call = receiver.recv() => match call {
                        Some(call) => call,
                        None => break,
                    },
                    _ = tokio::time::sleep(std::time::Duration::from_millis(wait as u64)) => {
                        fire_alarm(&runtime, alarm, call_budget, after_write.as_ref()).await;
                        continue;
                    }
                }
            } else if let Some(idle) = hibernate_after {
                tokio::select! {
                    call = receiver.recv() => match call {
                        Some(call) => call,
                        None => break,
                    },
                    // Hibernation is just ending: the file is the state,
                    // and the host revives on the next touch.
                    _ = tokio::time::sleep(idle) => break,
                }
            } else {
                match receiver.recv().await {
                    Some(call) => call,
                    None => break,
                }
            };

            let result = guarded_dispatch(
                &runtime,
                &call.method,
                call.payload,
                call_budget,
                after_write.as_ref(),
            )
            .await;

            // A caller that stopped waiting is its own problem; the state
            // change it asked for has already happened either way.
            let _ = call.reply.send(result);
        }
    });

    ObjectHandle { sender }
}

/// Runs one due alarm: cleared before dispatch, so a handler that sets the
/// next alarm is not clobbered afterwards. An alarm is best-effort work the
/// object asked itself for; its failure is logged, never propagated.
async fn fire_alarm(
    runtime: &ActiasRuntime,
    alarm: crate::extensions::objects::PendingAlarm,
    call_budget: Option<u64>,
    after_write: Option<&AfterWrite>,
) {
    if let Some(cell) = runtime.app_data_ref::<crate::extensions::objects::AlarmCell>() {
        *cell.0.borrow_mut() = None;
    }
    if let Some(cell) = runtime.app_data_ref::<crate::storage::StorageCell>()
        && let Err(error) = cell.0.borrow_mut().clear_alarm()
    {
        actias_common::tracing::warn!(%error, "alarm could not be cleared");
    }

    let result = guarded_dispatch(
        runtime,
        "__dispatch",
        serde_json::json!({
            "class": alarm.class,
            "method": "alarm",
            "args": [],
            "chain": [alarm.own_key],
        }),
        call_budget,
        after_write,
    )
    .await;

    if let Err(error) = result {
        actias_common::tracing::warn!(%error, "object alarm failed");
    }
}

/// One dispatched call, fully guarded: its own budget, its own
/// transaction (a failed method persists nothing partial), and the
/// checkpoint before any caller hears the result.
async fn guarded_dispatch(
    runtime: &ActiasRuntime,
    method: &str,
    payload: serde_json::Value,
    call_budget: Option<u64>,
    after_write: Option<&AfterWrite>,
) -> Result<serde_json::Value, ObjectError> {
    let has_storage = runtime
        .app_data_ref::<crate::storage::StorageCell>()
        .is_some();

    if has_storage
        && let Some(cell) = runtime.app_data_ref::<crate::storage::StorageCell>()
        && let Err(error) = cell.0.borrow_mut().begin()
    {
        return Err(ObjectError::Call(format!(
            "The call's transaction could not open: {error}"
        )));
    }

    if let Some(seconds) = call_budget {
        runtime.begin_call_budget(seconds);
    }
    let result = dispatch(runtime, method, payload).await;
    runtime.end_call_budget();

    if has_storage && let Some(cell) = runtime.app_data_ref::<crate::storage::StorageCell>() {
        let mut storage = cell.0.borrow_mut();

        match &result {
            Ok(_) => {
                if let Err(error) = storage.commit() {
                    return Err(ObjectError::Call(format!(
                        "The call's writes could not commit: {error}"
                    )));
                }
            }
            Err(_) => {
                if let Err(error) = storage.rollback() {
                    actias_common::tracing::warn!(%error, "rollback failed");
                }

                // The rolled-back row is the truth; the in-memory alarm
                // must not outlive an alarm the failed method set.
                let persisted =
                    storage
                        .load_alarm()
                        .ok()
                        .flatten()
                        .map(
                            |(due_ms, class, own_key)| crate::extensions::objects::PendingAlarm {
                                due_ms,
                                class,
                                own_key,
                            },
                        );
                drop(storage);
                if let Some(cell) = runtime.app_data_ref::<crate::extensions::objects::AlarmCell>()
                {
                    *cell.0.borrow_mut() = persisted;
                }
            }
        }
    }

    // The handler is done; give storage its flush moment before the
    // caller hears anything.
    if let Some(cell) = runtime.app_data_ref::<crate::storage::StorageCell>()
        && let Err(error) = cell.0.borrow_mut().checkpoint()
    {
        actias_common::tracing::warn!(%error, "object storage checkpoint failed");
    }

    // The output gate: a call that wrote does not answer until the write
    // has also left the building.
    if let Some(after_write) = after_write {
        let advanced = {
            let Some(cell) = runtime.app_data_ref::<crate::storage::StorageCell>() else {
                return result;
            };
            let current = cell.0.borrow_mut().total_changes().unwrap_or(0);
            let mark = runtime
                .app_data_ref::<ShipMark>()
                .map(|mark| mark.0.get())
                .unwrap_or(0);
            if current != mark {
                if let Some(ship_mark) = runtime.app_data_ref::<ShipMark>() {
                    ship_mark.0.set(current);
                }
                true
            } else {
                false
            }
        };
        if advanced {
            after_write().await;
        }
    }

    result
}

/// Extends a call chain onto `key`, refusing cycles.
///
/// Every routed call carries the keys already on its stack; a target that
/// appears there would deadlock on its own busy mailbox, so it is refused
/// loudly instead.
///
/// # Errors
/// Returns the cycle spelled out, for the script author.
pub fn extend_call_chain(chain: &[String], key: &str) -> Result<Vec<String>, String> {
    if chain.iter().any(|entry| entry == key) {
        return Err(format!(
            "Reentrant object call refused: {} -> {key} would deadlock.",
            chain.join(" -> "),
        ));
    }

    let mut child = chain.to_vec();
    child.push(key.to_owned());
    Ok(child)
}

/// Runs one method against the vm: json in, json out.
async fn dispatch(
    runtime: &ActiasRuntime,
    method: &str,
    payload: serde_json::Value,
) -> Result<serde_json::Value, ObjectError> {
    let function: mlua::Function = runtime
        .globals()
        .get(method)
        .map_err(|_| ObjectError::Call(format!("Object has no method '{method}'.")))?;

    let argument = runtime
        .to_value(&payload)
        .map_err(|e| ObjectError::Call(e.to_string()))?;

    let value: mlua::Value = function
        .call_async(argument)
        .await
        .map_err(|e| ObjectError::Call(e.to_string()))?;

    runtime
        .from_value(value)
        .map_err(|e| ObjectError::Call(e.to_string()))
}

/// The registry of live objects on this node, keyed by object identity
/// (never by revision: identity is what storage will hang off).
#[derive(Default)]
pub struct ObjectHost {
    tasks: Mutex<HashMap<String, (String, ObjectHandle)>>,
}

impl ObjectHost {
    /// The handle for `id`, spawning its task on first use. A changed
    /// `marker` (the revision the vm should embody) evicts the old task
    /// and builds a fresh one, so a republish never serves stale code and
    /// retired vms do not accumulate.
    ///
    /// The factory runs under the registry lock, so two racing callers can
    /// never both build a vm for one object; correctness first, and object
    /// construction is rare next to calls.
    ///
    /// # Errors
    /// Returns whatever the factory failed with; nothing is registered.
    pub async fn get_or_spawn<F, Fut>(
        &self,
        id: &str,
        marker: &str,
        factory: F,
    ) -> mlua::Result<ObjectHandle>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = mlua::Result<(ActiasRuntime, TaskOptions)>>,
    {
        let mut tasks = self.tasks.lock().await;

        // A hibernated task's sender reads closed; it respawns exactly
        // like a retired revision would.
        if let Some((held, handle)) = tasks.get(id)
            && held == marker
            && !handle.sender.is_closed()
        {
            return Ok(handle.clone());
        }

        let (runtime, options) = factory().await?;
        let handle = spawn_object_task(runtime, options);
        tasks.insert(id.to_owned(), (marker.to_owned(), handle.clone()));

        Ok(handle)
    }

    /// How many objects currently have live tasks; hibernated ones do
    /// not count.
    pub async fn resident_count(&self) -> usize {
        self.tasks
            .lock()
            .await
            .values()
            .filter(|(_, handle)| !handle.sender.is_closed())
            .count()
    }

    /// Whether the object currently has a live task; a hibernated one
    /// reads as absent.
    pub async fn is_resident(&self, id: &str) -> bool {
        self.tasks
            .lock()
            .await
            .get(id)
            .is_some_and(|(_, handle)| !handle.sender.is_closed())
    }

    /// Drops an object's registry entry; its task ends once in-flight
    /// callers finish. The next access builds a fresh vm.
    pub async fn evict(&self, id: &str) {
        self.tasks.lock().await.remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::bundle::{Bundle, File};
    use crate::proto::kv_service::kv_service_client::KvServiceClient;
    use crate::proto::script_service::{Revision, Script};
    use crate::runtime::PreparedRevision;
    use std::sync::Arc;

    /// A pinned runtime whose entry point is `source`; clients are
    /// unconnectable, so tests exercise only the vm.
    async fn runtime_with(source: &str) -> ActiasRuntime {
        runtime_with_files(&[("main.lua", source)]).await
    }

    /// Like [`runtime_with`] but with a whole bundle of files.
    async fn runtime_with_files(files: &[(&str, &str)]) -> ActiasRuntime {
        let revision = Revision {
            bundle: Some(Bundle {
                entry_point: "main.lua".to_owned(),
                files: files
                    .iter()
                    .map(|(path, content)| File {
                        file_path: (*path).to_owned(),
                        content: content.as_bytes().to_vec(),
                        ..Default::default()
                    })
                    .collect(),
            }),
            ..Default::default()
        };
        let prepared =
            Arc::new(PreparedRevision::prepare(Script::default(), revision).expect("prepares"));

        let channel = tonic::transport::Channel::from_static("http://127.0.0.1:1").connect_lazy();
        let egress = crate::egress::EgressClient::new(crate::egress::EgressPolicy::new([], false))
            .expect("egress builds");

        let runtime = ActiasRuntime::new(
            prepared,
            KvServiceClient::new(channel),
            egress,
            None,
            None,
            None,
        )
        .await
        .expect("runtime builds");

        // A real await point for the interleaving test: without the input
        // gate, a second call could run while the first sleeps here.
        runtime
            .globals()
            .set(
                "sleep_ms",
                runtime
                    .create_async_function(|_, ms: u64| async move {
                        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                        Ok(())
                    })
                    .expect("function builds"),
            )
            .expect("global sets");

        runtime
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent_calls_never_interleave() {
        let runtime = runtime_with(
            r#"
            journal = {}
            function slow(tag)
                table.insert(journal, tag .. ":start")
                sleep_ms(20)
                table.insert(journal, tag .. ":end")
                return journal
            end
            function read()
                return journal
            end
            "#,
        )
        .await;
        let handle = spawn_object_task(runtime, TaskOptions::default());

        let (a, b) = tokio::join!(
            handle.call("slow", serde_json::json!("a")),
            handle.call("slow", serde_json::json!("b")),
        );
        a.expect("first call succeeds");
        b.expect("second call succeeds");

        let journal = handle
            .call("read", serde_json::Value::Null)
            .await
            .expect("journal reads");
        let journal: Vec<String> = serde_json::from_value(journal).expect("a string list");

        assert_eq!(journal.len(), 4);
        // Whatever the arrival order, every start is immediately followed
        // by its own end: the await inside the handler let nothing in.
        for pair in journal.chunks(2) {
            let tag = pair[0].strip_suffix(":start").expect("a start marker");
            assert_eq!(pair[1], format!("{tag}:end"), "interleaved: {journal:?}");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn state_lives_as_long_as_the_task() {
        let runtime = runtime_with(
            r#"
            count = 0
            function bump()
                count = count + 1
                return count
            end
            "#,
        )
        .await;
        let handle = spawn_object_task(runtime, TaskOptions::default());

        for expected in 1..=3 {
            let value = handle
                .call("bump", serde_json::Value::Null)
                .await
                .expect("bump succeeds");
            assert_eq!(value, serde_json::json!(expected));
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_unknown_method_is_a_call_error() {
        let runtime = runtime_with("x = 1").await;
        let handle = spawn_object_task(runtime, TaskOptions::default());

        let error = handle
            .call("nope", serde_json::Value::Null)
            .await
            .expect_err("missing methods must error");

        assert!(matches!(error, ObjectError::Call(_)));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_host_reuses_one_task_per_id() {
        let host = ObjectHost::default();

        let first = host
            .get_or_spawn("obj-1", "r1", || async {
                Ok((
                    runtime_with("count = 0 function bump() count = count + 1 return count end")
                        .await,
                    TaskOptions::default(),
                ))
            })
            .await
            .expect("spawns");
        first
            .call("bump", serde_json::Value::Null)
            .await
            .expect("bump");

        // The second resolve must reach the same vm, not a fresh one.
        let second = host
            .get_or_spawn("obj-1", "r1", || async { panic!("factory must not rerun") })
            .await
            .expect("reuses");
        let value = second
            .call("bump", serde_json::Value::Null)
            .await
            .expect("bump");
        assert_eq!(value, serde_json::json!(2));
    }

    /// The whole object story in one process: two separate "request" vms
    /// route method calls through one host, whose pinned vm holds state.
    #[tokio::test(flavor = "multi_thread")]
    async fn object_state_is_shared_across_request_vms() {
        use crate::extensions::objects::{ObjectRouter, ObjectTarget};

        const SOURCE: &str = r#"
            local Counter = object "Counter" {
                bump = function(state, amount)
                    state.total = (state.total or 0) + amount
                    return state.total
                end,
            }

            on "fetch" (function(request)
                local counter = Counter:get("main")
                return { body = counter:bump(request.amount) }
            end)
        "#;

        let host = Arc::new(ObjectHost::default());
        let router: ObjectRouter = Arc::new(move |target: ObjectTarget| {
            let host = host.clone();
            Box::pin(async move {
                let id = format!("{}/{}", target.class, target.name);
                let handle = host
                    .get_or_spawn(&id, "r1", || async {
                        Ok((runtime_with(SOURCE).await, TaskOptions::default()))
                    })
                    .await
                    .map_err(|e| e.to_string())?;

                handle
                    .call(
                        "__dispatch",
                        serde_json::json!({
                            "class": target.class,
                            "method": target.method,
                            "args": target.arguments,
                        }),
                    )
                    .await
                    .map_err(|e| e.to_string())
            })
        });

        let mut bodies = Vec::new();
        for amount in [3i64, 4] {
            // A fresh vm per request, exactly like the worker's hot path.
            let runtime = runtime_with(SOURCE).await;
            runtime.set_app_data::<ObjectRouter>(router.clone());

            let listener = runtime
                .listener(ActiasRuntime::FETCH_EVENT)
                .expect("handler registered");
            let response: mlua::Value = listener
                .call_async(
                    runtime
                        .to_value(&serde_json::json!({ "amount": amount }))
                        .expect("request converts"),
                )
                .await
                .expect("handler runs");

            let response: serde_json::Value =
                runtime.from_value(response).expect("response converts");
            bodies.push(response["body"].clone());
        }

        // The second request observed the first one's mutation.
        assert_eq!(bodies, vec![serde_json::json!(3), serde_json::json!(7)]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_contract_without_the_class_refuses_the_declaration() {
        use crate::proto::script_service::{Capabilities, ScriptConfig};

        let revision = Revision {
            bundle: Some(Bundle {
                entry_point: "main.lua".to_owned(),
                files: vec![File {
                    file_path: "main.lua".to_owned(),
                    content: br#"local C = object "Sneaky" {}"#.to_vec(),
                    ..Default::default()
                }],
            }),
            script_config: Some(ScriptConfig {
                id: String::new(),
                entry_point: "main.lua".to_owned(),
                includes: vec![],
                ignore: vec![],
                capabilities: Some(Capabilities {
                    kv: vec![],
                    events: vec![],
                    secrets: vec![],
                    objects: vec![],
                    databases: vec![],
                }),
            }),
            ..Default::default()
        };
        let prepared =
            Arc::new(PreparedRevision::prepare(Script::default(), revision).expect("prepares"));

        let channel = tonic::transport::Channel::from_static("http://127.0.0.1:1").connect_lazy();
        let egress = crate::egress::EgressClient::new(crate::egress::EgressPolicy::new([], false))
            .expect("egress builds");

        let result = ActiasRuntime::new(
            prepared,
            KvServiceClient::new(channel),
            egress,
            None,
            None,
            None,
        )
        .await;

        let Err(error) = result else {
            panic!("a class outside the contract must be refused");
        };
        assert!(error.to_string().contains("Object class"), "{error}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_changed_marker_respawns_the_vm() {
        let host = ObjectHost::default();
        let source = "count = 0 function bump() count = count + 1 return count end";

        let first = host
            .get_or_spawn("obj-1", "rev-1", || async {
                Ok((runtime_with(source).await, TaskOptions::default()))
            })
            .await
            .expect("spawns");
        first
            .call("bump", serde_json::Value::Null)
            .await
            .expect("bump");

        // A new revision must reach fresh code (here: fresh state), not
        // the retired vm.
        let second = host
            .get_or_spawn("obj-1", "rev-2", || async {
                Ok((runtime_with(source).await, TaskOptions::default()))
            })
            .await
            .expect("respawns");
        let value = second
            .call("bump", serde_json::Value::Null)
            .await
            .expect("bump");
        assert_eq!(value, serde_json::json!(1));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_runaway_method_times_out_and_the_vm_survives() {
        let runtime = runtime_with(
            r#"
            function spin() while true do end end
            function ping() return 1 end
            "#,
        )
        .await;
        let handle = spawn_object_task(
            runtime,
            TaskOptions {
                call_budget: Some(1),
                ..Default::default()
            },
        );

        let error = handle
            .call("spin", serde_json::Value::Null)
            .await
            .expect_err("the runaway must time out");
        assert!(matches!(error, ObjectError::Call(_)), "{error}");

        // The budget was per call: the vm answers the next caller.
        let value = handle
            .call("ping", serde_json::Value::Null)
            .await
            .expect("the vm survives its runaway");
        assert_eq!(value, serde_json::json!(1));
    }

    #[test]
    fn a_cycle_in_the_call_chain_is_refused() {
        let chain = extend_call_chain(&[], "a").expect("first hop");
        let chain = extend_call_chain(&chain, "b").expect("second hop");

        assert_eq!(chain, vec!["a".to_owned(), "b".to_owned()]);

        let refused = extend_call_chain(&chain, "a").expect_err("a -> b -> a must refuse");
        assert!(refused.contains("a -> b -> a"), "{refused}");
    }

    /// The restart story end to end at unit scale: a fresh task over the
    /// same file resumes exactly where the dead one stopped.
    #[tokio::test(flavor = "multi_thread")]
    async fn sql_state_survives_a_task_replacement() {
        const SOURCE: &str = r#"
            local Keeper = object "Keeper" {
                bump = function(state)
                    state.sql:exec("CREATE TABLE IF NOT EXISTS hits (at INTEGER)")
                    state.sql:exec("INSERT INTO hits VALUES (?)", { 1 })
                    return state.sql:query_one("SELECT COUNT(*) AS n FROM hits").n
                end,
            }
        "#;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("keeper.db");

        let call = serde_json::json!({ "class": "Keeper", "method": "bump", "args": [] });

        let first = spawn_object_task(
            runtime_with(SOURCE).await,
            TaskOptions {
                storage: Some(crate::storage::SqliteStorage::open(&path).expect("opens")),
                ..Default::default()
            },
        );
        assert_eq!(
            first.call("__dispatch", call.clone()).await.expect("bump"),
            serde_json::json!(1)
        );
        assert_eq!(
            first.call("__dispatch", call.clone()).await.expect("bump"),
            serde_json::json!(2)
        );
        drop(first);

        // "The worker restarted": nothing survives but the file.
        let second = spawn_object_task(
            runtime_with(SOURCE).await,
            TaskOptions {
                storage: Some(crate::storage::SqliteStorage::open(&path).expect("reopens")),
                ..Default::default()
            },
        );
        assert_eq!(
            second.call("__dispatch", call.clone()).await.expect("bump"),
            serde_json::json!(3)
        );
    }

    /// The sql product face: `database "name"` reads and writes durably
    /// through the same machinery objects use.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_database_is_an_object_wearing_sql() {
        use crate::extensions::objects::{ObjectRouter, ObjectTarget};

        const SOURCE: &str = r#"
            local db = database "main"

            on "fetch" (function(request)
                db:exec("CREATE TABLE IF NOT EXISTS visits (at INTEGER)")
                db:exec("INSERT INTO visits VALUES (?)", { 1 })
                return { body = db:query_one("SELECT COUNT(*) AS n FROM visits").n }
            end)
        "#;

        let dir = tempfile::tempdir().expect("tempdir");
        let data = dir.path().to_path_buf();
        let host = Arc::new(ObjectHost::default());

        let router: ObjectRouter = Arc::new(move |target: ObjectTarget| {
            let host = host.clone();
            let data = data.clone();
            Box::pin(async move {
                let id = format!("{}/{}", target.class, target.name);
                let file = data.join("db.sqlite");
                let handle = host
                    .get_or_spawn(&id, "r1", || async {
                        let storage = crate::storage::SqliteStorage::open(&file)
                            .map_err(mlua::Error::RuntimeError)?;
                        Ok((
                            runtime_with(SOURCE).await,
                            TaskOptions {
                                storage: Some(storage),
                                ..Default::default()
                            },
                        ))
                    })
                    .await
                    .map_err(|e| e.to_string())?;

                handle
                    .call(
                        "__dispatch",
                        serde_json::json!({
                            "class": target.class,
                            "method": target.method,
                            "args": target.arguments,
                        }),
                    )
                    .await
                    .map_err(|e| e.to_string())
            })
        });

        let mut counts = Vec::new();
        for _ in 0..2 {
            let runtime = runtime_with(SOURCE).await;
            runtime.set_app_data::<ObjectRouter>(router.clone());

            let listener = runtime
                .listener(ActiasRuntime::FETCH_EVENT)
                .expect("handler registered");
            let response: mlua::Value = listener
                .call_async(runtime.to_value(&serde_json::json!({})).expect("converts"))
                .await
                .expect("handler runs");
            let response: serde_json::Value =
                runtime.from_value(response).expect("response converts");
            counts.push(response["body"].clone());
        }

        assert_eq!(counts, vec![serde_json::json!(1), serde_json::json!(2)]);
    }

    const LIFECYCLE_SOURCE: &str = r#"
        local Keeper = object "Keeper" {
            init = function(state)
                state.sql:exec("CREATE TABLE births (at INTEGER)")
                state.sql:exec("INSERT INTO births VALUES (1)")
                state.sql:exec("CREATE TABLE IF NOT EXISTS alarms (at INTEGER)")
            end,

            poke = function(state, duration)
                state:set_alarm(duration)
                return true
            end,

            alarm = function(state)
                state.sql:exec("INSERT INTO alarms VALUES (?)", { state.now() })
            end,

            births = function(state)
                return state.sql:query_one("SELECT COUNT(*) AS n FROM births").n
            end,

            alarms = function(state)
                return state.sql:query_one("SELECT COUNT(*) AS n FROM alarms").n
            end,
        }
    "#;

    fn keeper_call(method: &str, args: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "class": "Keeper", "method": method, "args": args })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn init_runs_exactly_once_per_object() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("keeper.db");

        let first = spawn_object_task(
            runtime_with(LIFECYCLE_SOURCE).await,
            TaskOptions {
                storage: Some(crate::storage::SqliteStorage::open(&path).expect("opens")),
                ..Default::default()
            },
        );
        for _ in 0..2 {
            assert_eq!(
                first
                    .call("__dispatch", keeper_call("births", serde_json::json!([])))
                    .await
                    .expect("births"),
                serde_json::json!(1),
                "init must run once, not per call"
            );
        }
        drop(first);

        // A replacement task over the same file must see the file as
        // initialized, never rerunning init.
        let second = spawn_object_task(
            runtime_with(LIFECYCLE_SOURCE).await,
            TaskOptions {
                storage: Some(crate::storage::SqliteStorage::open(&path).expect("reopens")),
                ..Default::default()
            },
        );
        assert_eq!(
            second
                .call("__dispatch", keeper_call("births", serde_json::json!([])))
                .await
                .expect("births"),
            serde_json::json!(1),
            "init must not rerun after a restart"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_alarm_fires_without_any_request() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("keeper.db");

        let handle = spawn_object_task(
            runtime_with(LIFECYCLE_SOURCE).await,
            TaskOptions {
                storage: Some(crate::storage::SqliteStorage::open(&path).expect("opens")),
                ..Default::default()
            },
        );

        handle
            .call(
                "__dispatch",
                keeper_call("poke", serde_json::json!(["200ms"])),
            )
            .await
            .expect("poke");

        // No calls happen here; only the object's own clock.
        tokio::time::sleep(std::time::Duration::from_millis(900)).await;

        assert_eq!(
            handle
                .call("__dispatch", keeper_call("alarms", serde_json::json!([])))
                .await
                .expect("alarms"),
            serde_json::json!(1),
            "the alarm must have fired on its own"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_persisted_alarm_survives_a_task_replacement() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("keeper.db");

        let first = spawn_object_task(
            runtime_with(LIFECYCLE_SOURCE).await,
            TaskOptions {
                storage: Some(crate::storage::SqliteStorage::open(&path).expect("opens")),
                ..Default::default()
            },
        );
        first
            .call(
                "__dispatch",
                keeper_call("poke", serde_json::json!(["300ms"])),
            )
            .await
            .expect("poke");
        // The task dies before the alarm is due; only the file remembers.
        drop(first);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let second = spawn_object_task(
            runtime_with(LIFECYCLE_SOURCE).await,
            TaskOptions {
                storage: Some(crate::storage::SqliteStorage::open(&path).expect("reopens")),
                ..Default::default()
            },
        );
        tokio::time::sleep(std::time::Duration::from_millis(900)).await;

        assert_eq!(
            second
                .call("__dispatch", keeper_call("alarms", serde_json::json!([])))
                .await
                .expect("alarms"),
            serde_json::json!(1),
            "the re-armed alarm must fire after the replacement"
        );
    }

    /// Migrations apply at first touch, exactly once per database, and a
    /// respawn over the same file never reapplies them (the CREATE would
    /// fail if it did).
    #[tokio::test(flavor = "multi_thread")]
    async fn migrations_apply_once_at_first_touch() {
        const MAIN: &str = r#"local db = database "main""#;
        const MIGRATION: &str = "CREATE TABLE visits (at INTEGER);";
        let files = [
            ("main.lua", MAIN),
            ("migrations/main/0001_init.sql", MIGRATION),
        ];

        let call = |method: &str, args: serde_json::Value| {
            serde_json::json!({
                "class": "__database", "name": "main", "method": method, "args": args,
            })
        };

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("main.db");

        let first = spawn_object_task(
            runtime_with_files(&files).await,
            TaskOptions {
                storage: Some(crate::storage::SqliteStorage::open(&path).expect("opens")),
                ..Default::default()
            },
        );
        // The migrated table exists on the very first statement.
        first
            .call(
                "__dispatch",
                call("exec", serde_json::json!(["INSERT INTO visits VALUES (1)"])),
            )
            .await
            .expect("the migration created the table");
        drop(first);

        // A fresh vm over the same file skips the applied migration; a
        // reapply would fail on the bare CREATE.
        let second = spawn_object_task(
            runtime_with_files(&files).await,
            TaskOptions {
                storage: Some(crate::storage::SqliteStorage::open(&path).expect("reopens")),
                ..Default::default()
            },
        );
        let count = second
            .call(
                "__dispatch",
                call(
                    "query_one",
                    serde_json::json!(["SELECT COUNT(*) AS n FROM visits"]),
                ),
            )
            .await
            .expect("the respawn served without reapplying");
        assert_eq!(count, serde_json::json!({ "n": 1 }));
    }

    /// The transaction guard: a method that errors after writing must
    /// leave nothing behind.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_failed_method_persists_nothing_partial() {
        const SOURCE: &str = r#"
            local Ledger = object "Ledger" {
                init = function(state)
                    state.sql:exec("CREATE TABLE entries (n INTEGER)")
                end,

                good = function(state)
                    state.sql:exec("INSERT INTO entries VALUES (1)")
                    return true
                end,

                bad = function(state)
                    state.sql:exec("INSERT INTO entries VALUES (2)")
                    error("halfway failure")
                end,

                count = function(state)
                    return state.sql:query_one("SELECT COUNT(*) AS n FROM entries").n
                end,
            }
        "#;
        let call =
            |method: &str| serde_json::json!({ "class": "Ledger", "method": method, "args": [] });

        let dir = tempfile::tempdir().expect("tempdir");
        let handle = spawn_object_task(
            runtime_with(SOURCE).await,
            TaskOptions {
                storage: Some(
                    crate::storage::SqliteStorage::open(&dir.path().join("ledger.db"))
                        .expect("opens"),
                ),
                ..Default::default()
            },
        );

        handle.call("__dispatch", call("good")).await.expect("good");
        handle
            .call("__dispatch", call("bad"))
            .await
            .expect_err("the failure must surface");

        // The failed method's insert rolled back; only the good one stands.
        assert_eq!(
            handle
                .call("__dispatch", call("count"))
                .await
                .expect("count"),
            serde_json::json!(1)
        );

        // The vm is healthy after the rollback.
        handle
            .call("__dispatch", call("good"))
            .await
            .expect("good again");
        assert_eq!(
            handle
                .call("__dispatch", call("count"))
                .await
                .expect("count"),
            serde_json::json!(2)
        );
    }

    /// db:batch is atomic because a batch is one call, one transaction.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_batch_with_a_bad_statement_is_all_or_nothing() {
        const SOURCE: &str = r#"
            local db = database "main"

            on "fetch" (function(request)
                if request.mode == "bad" then
                    db:batch({
                        { "CREATE TABLE IF NOT EXISTS t (n INTEGER)" },
                        { "INSERT INTO t VALUES (?)", { 1 } },
                        { "INSERT INTO no_such_table VALUES (1)" },
                    })
                    return { body = "unreachable" }
                end
                db:exec("CREATE TABLE IF NOT EXISTS t (n INTEGER)")
                return { body = db:query_one("SELECT COUNT(*) AS n FROM t").n }
            end)
        "#;

        use crate::extensions::objects::{ObjectRouter, ObjectTarget};
        let dir = tempfile::tempdir().expect("tempdir");
        let data = dir.path().to_path_buf();
        let host = Arc::new(ObjectHost::default());

        let router: ObjectRouter = Arc::new(move |target: ObjectTarget| {
            let host = host.clone();
            let data = data.clone();
            Box::pin(async move {
                let id = format!("{}/{}", target.class, target.name);
                let file = data.join("main.db");
                let handle = host
                    .get_or_spawn(&id, "r1", || async {
                        let storage = crate::storage::SqliteStorage::open(&file)
                            .map_err(mlua::Error::RuntimeError)?;
                        Ok((
                            runtime_with(SOURCE).await,
                            TaskOptions {
                                storage: Some(storage),
                                ..Default::default()
                            },
                        ))
                    })
                    .await
                    .map_err(|e| e.to_string())?;
                handle
                    .call(
                        "__dispatch",
                        serde_json::json!({
                            "class": target.class,
                            "method": target.method,
                            "args": target.arguments,
                        }),
                    )
                    .await
                    .map_err(|e| e.to_string())
            })
        });

        let run = |mode: &'static str, router: ObjectRouter| async move {
            let runtime = runtime_with(SOURCE).await;
            runtime.set_app_data::<ObjectRouter>(router);
            let listener = runtime
                .listener(ActiasRuntime::FETCH_EVENT)
                .expect("handler registered");
            let value: Result<mlua::Value, _> = listener
                .call_async(
                    runtime
                        .to_value(&serde_json::json!({ "mode": mode }))
                        .expect("converts"),
                )
                .await;
            value.map(|value| {
                let response: serde_json::Value = runtime.from_value(value).expect("converts");
                response["body"].clone()
            })
        };

        run("bad", router.clone())
            .await
            .expect_err("the bad batch must fail");
        // Nothing from the failed batch survived, including its insert.
        assert_eq!(
            run("count", router.clone()).await.expect("count"),
            serde_json::json!(0)
        );
    }

    /// Hibernation: an idle task ends itself, and the host revives the
    /// object from its file on the next touch, state intact.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_idle_object_hibernates_and_revives_with_its_state() {
        const SOURCE: &str = r#"
            local Keeper = object "Keeper" {
                init = function(state)
                    state.sql:exec("CREATE TABLE hits (at INTEGER)")
                end,
                bump = function(state)
                    state.sql:exec("INSERT INTO hits VALUES (1)")
                    return state.sql:query_one("SELECT COUNT(*) AS n FROM hits").n
                end,
            }
        "#;
        let call = serde_json::json!({ "class": "Keeper", "method": "bump", "args": [] });

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("keeper.db");
        let host = ObjectHost::default();

        let spawn = |path: std::path::PathBuf| async move {
            Ok((
                runtime_with(SOURCE).await,
                TaskOptions {
                    storage: Some(crate::storage::SqliteStorage::open(&path).expect("opens")),
                    hibernate_after: Some(std::time::Duration::from_millis(150)),
                    ..Default::default()
                },
            ))
        };

        let first = host
            .get_or_spawn("keeper", "r1", || spawn(path.clone()))
            .await
            .expect("spawns");
        assert_eq!(
            first.call("__dispatch", call.clone()).await.expect("bump"),
            serde_json::json!(1)
        );

        // Long past the idle window: the task must have ended itself.
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        let gone = first.call("__dispatch", call.clone()).await;
        assert!(
            matches!(gone, Err(ObjectError::Gone)),
            "the idle task must be gone: {gone:?}"
        );

        // The next touch through the host revives from the file.
        let revived = host
            .get_or_spawn("keeper", "r1", || spawn(path.clone()))
            .await
            .expect("revives");
        assert_eq!(
            revived.call("__dispatch", call).await.expect("bump"),
            serde_json::json!(2),
            "revival must resume the durable state"
        );
    }

    /// An object holding a pending alarm refuses to sleep: hibernating
    /// past the alarm would silently drop it.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_alarm_holding_object_refuses_to_sleep() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("keeper.db");

        let handle = spawn_object_task(
            runtime_with(LIFECYCLE_SOURCE).await,
            TaskOptions {
                storage: Some(crate::storage::SqliteStorage::open(&path).expect("opens")),
                hibernate_after: Some(std::time::Duration::from_millis(100)),
                ..Default::default()
            },
        );

        // The alarm is due at 400ms, well past the 100ms idle window; the
        // vm must stay warm to fire it rather than sleeping first. Once
        // fired there is nothing keeping it awake, so by 700ms it has both
        // fired the alarm and hibernated.
        handle
            .call(
                "__dispatch",
                keeper_call("poke", serde_json::json!(["400ms"])),
            )
            .await
            .expect("poke");
        tokio::time::sleep(std::time::Duration::from_millis(700)).await;

        let gone = handle
            .call("__dispatch", keeper_call("alarms", serde_json::json!([])))
            .await;
        assert!(matches!(gone, Err(ObjectError::Gone)), "{gone:?}");

        // The file is the witness: one alarm row means the vm was still
        // warm at 400ms; sleeping at 100ms would have left zero.
        let mut storage = crate::storage::SqliteStorage::open(&path).expect("reopens");
        let rows = storage
            .query("SELECT COUNT(*) AS n FROM alarms", &[])
            .expect("reads");
        assert_eq!(rows, vec![serde_json::json!({ "n": 1 })]);
    }

    /// The cron machinery end to end in one vm: ensure arms, the alarm
    /// fires the listener and re-arms itself, forever.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_cron_object_fires_its_listener_on_schedule() {
        const EVENT: &str = "cron:* * * * * *";
        // The listener yields (an async platform call) on purpose: firing
        // must survive handlers that await, which a lua pcall cannot.
        const SOURCE: &str = r#"
            marks = 0
            on "cron:* * * * * *" (function(event)
                sleep_ms(5)
                marks = marks + 1
            end)
            function get_marks() return marks end
        "#;

        let dir = tempfile::tempdir().expect("tempdir");
        let handle = spawn_object_task(
            runtime_with(SOURCE).await,
            TaskOptions {
                storage: Some(
                    crate::storage::SqliteStorage::open(&dir.path().join("cron.db"))
                        .expect("opens"),
                ),
                ..Default::default()
            },
        );

        handle
            .call(
                "__dispatch",
                serde_json::json!({
                    "class": "__cron", "name": EVENT, "method": "ensure", "args": [EVENT],
                }),
            )
            .await
            .expect("ensure arms");

        // An every-second schedule (clamped to 1s minimum) must have fired
        // at least twice in 2.6s, proving the alarm re-arms itself.
        tokio::time::sleep(std::time::Duration::from_millis(2600)).await;
        let marks = handle
            .call("get_marks", serde_json::Value::Null)
            .await
            .expect("marks read");
        let marks = marks.as_i64().unwrap_or(0);
        assert!(marks >= 2, "the schedule must self-perpetuate: {marks}");
    }

    #[test]
    fn cron_expressions_read_both_shapes() {
        use crate::extensions::objects::cron_delay_ms;

        // Six-field (with seconds) and classic five-field both parse; the
        // clamp keeps even every-second schedules a real sleep.
        assert!(cron_delay_ms("cron:* * * * * *").expect("six-field") >= 1000);
        assert!(cron_delay_ms("cron:*/5 * * * *").expect("five-field") >= 1000);
        assert!(cron_delay_ms("cron:not a schedule").is_err());
    }

    /// The output gate: only calls that wrote pay it, and it has run by
    /// the time the caller has its answer.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_after_write_gate_fires_for_writes_only() {
        const SOURCE: &str = r#"
            local Keeper = object "Keeper" {
                init = function(state)
                    state.sql:exec("CREATE TABLE t (n INTEGER)")
                end,
                put = function(state)
                    state.sql:exec("INSERT INTO t VALUES (1)")
                end,
                peek = function(state)
                    return state.sql:query_one("SELECT COUNT(*) AS n FROM t").n
                end,
            }
        "#;
        let call =
            |method: &str| serde_json::json!({ "class": "Keeper", "method": method, "args": [] });

        let shipped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = shipped.clone();
        let dir = tempfile::tempdir().expect("tempdir");

        let handle = spawn_object_task(
            runtime_with(SOURCE).await,
            TaskOptions {
                storage: Some(
                    crate::storage::SqliteStorage::open(&dir.path().join("k.db")).expect("opens"),
                ),
                after_write: Some(Arc::new(move || {
                    let observed = observed.clone();
                    Box::pin(async move {
                        observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    })
                })),
                ..Default::default()
            },
        );

        // init + insert happen on the first call: one gate.
        handle.call("__dispatch", call("put")).await.expect("put");
        assert_eq!(shipped.load(std::sync::atomic::Ordering::SeqCst), 1);

        // A pure read moves nothing and pays nothing.
        handle.call("__dispatch", call("peek")).await.expect("peek");
        assert_eq!(shipped.load(std::sync::atomic::Ordering::SeqCst), 1);

        handle.call("__dispatch", call("put")).await.expect("put");
        assert_eq!(shipped.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mailbox_overhead_is_visible() {
        // Not an assertion, a measurement: the per-call cost of the mailbox
        // plus mlua's send-feature locking, recorded so the !Send-vm option
        // stays a data question. Run with --nocapture to read it.
        let runtime = runtime_with("function ping() return 1 end").await;
        let handle = spawn_object_task(runtime, TaskOptions::default());

        let rounds = 10_000u32;
        let start = std::time::Instant::now();
        for _ in 0..rounds {
            handle
                .call("ping", serde_json::Value::Null)
                .await
                .expect("ping");
        }
        let per_call = start.elapsed() / rounds;

        println!("mailbox call round trip: {per_call:?} per call over {rounds} calls");
    }
}
