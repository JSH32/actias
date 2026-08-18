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
pub fn spawn_object_task(runtime: ActiasRuntime, call_budget: Option<u64>) -> ObjectHandle {
    let (sender, mut receiver) = mpsc::channel::<ObjectCall>(MAILBOX_DEPTH);

    tokio::spawn(async move {
        // Popping only after the previous call finished is the input gate;
        // there is deliberately no concurrency inside this loop.
        while let Some(call) = receiver.recv().await {
            // Each call gets its own budget: a runaway method times out and
            // the vm survives for the next caller, instead of a lifetime
            // limit eventually killing a healthy object.
            if let Some(seconds) = call_budget {
                runtime.begin_call_budget(seconds);
            }
            let result = dispatch(&runtime, &call.method, call.payload).await;
            runtime.end_call_budget();

            // A caller that stopped waiting is its own problem; the state
            // change it asked for has already happened either way.
            let _ = call.reply.send(result);
        }
    });

    ObjectHandle { sender }
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
        Fut: Future<Output = mlua::Result<(ActiasRuntime, Option<u64>)>>,
    {
        let mut tasks = self.tasks.lock().await;

        if let Some((held, handle)) = tasks.get(id)
            && held == marker
        {
            return Ok(handle.clone());
        }

        let (runtime, call_budget) = factory().await?;
        let handle = spawn_object_task(runtime, call_budget);
        tasks.insert(id.to_owned(), (marker.to_owned(), handle.clone()));

        Ok(handle)
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
        let revision = Revision {
            bundle: Some(Bundle {
                entry_point: "main.lua".to_owned(),
                files: vec![File {
                    file_path: "main.lua".to_owned(),
                    content: source.as_bytes().to_vec(),
                    ..Default::default()
                }],
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
        let handle = spawn_object_task(runtime, None);

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
        let handle = spawn_object_task(runtime, None);

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
        let handle = spawn_object_task(runtime, None);

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
                    None,
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
                        Ok((runtime_with(SOURCE).await, None))
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
                Ok((runtime_with(source).await, None))
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
                Ok((runtime_with(source).await, None))
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
        let handle = spawn_object_task(runtime, Some(1));

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

    #[tokio::test(flavor = "multi_thread")]
    async fn mailbox_overhead_is_visible() {
        // Not an assertion, a measurement: the per-call cost of the mailbox
        // plus mlua's send-feature locking, recorded so the !Send-vm option
        // stays a data question. Run with --nocapture to read it.
        let runtime = runtime_with("function ping() return 1 end").await;
        let handle = spawn_object_task(runtime, None);

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
