//! The workflow journal on object storage: one append-only table in the
//! instance's own SQLite file. Everything a run ever did or decided is a
//! row here; replay is reading it back in order. The verbs (W3) fold
//! over this; this module owns only the substrate: schema, kinds,
//! append, cursor reads.
//!
//! Entries carry a format version from day one, the cheap insurance that
//! lets a later engine (or a continuation checkpoint) replace the replay
//! tail without a table migration.

use serde::{Deserialize, Serialize};

/// The journal schema's version cell value; moves like the queue's did
/// (v1 to v2 rebuild proved the mechanism).
const SCHEMA_VERSION: i64 = 2;

/// The current entry format; stamped per row, not per file, so a tail
/// written by newer code coexists with an older head.
pub const ENTRY_FORMAT: i64 = 1;

/// Sequence order IS execution order: the mailbox serializes appends
/// structurally, and AUTOINCREMENT keeps seq unique forever even if
/// rows were ever pruned.
const CREATE_JOURNAL: &str = "CREATE TABLE IF NOT EXISTS __actias_wf_journal (
        seq INTEGER PRIMARY KEY AUTOINCREMENT,
        at INTEGER NOT NULL,
        kind TEXT NOT NULL,
        data TEXT NOT NULL,
        format INTEGER NOT NULL
    )";

/// A run keeps the credentials it started with: `secret "name"` pins the
/// version its first resolution returned, and every later vm build
/// resolves exactly that version. Versions only; values never touch the
/// journal file, and the secret service keeps pinned versions resolvable
/// through rotation and delete.
const CREATE_SECRET_PINS: &str = "CREATE TABLE IF NOT EXISTS __actias_wf_secrets (
        name TEXT PRIMARY KEY,
        version INTEGER NOT NULL
    )";

/// Everything a journal row can record. Replay must understand every
/// kind it can meet; when the set or the row shapes change, the
/// version-cell ladder in [`ensure_schema`] migrates old files once,
/// visibly, exactly as the queue's v1 to v2 rebuild did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EntryKind {
    /// The run began: input, pinned revision id, random seed, engine.
    Started,
    /// A step is about to run its effect; the irreducible crash window
    /// opens here.
    Intent,
    /// The step's effect finished; replay returns this value instead of
    /// running the body again.
    Result,
    /// A sleep parked the run until a due time; the standard alarm row
    /// mirrors it.
    Timer,
    /// A signal arrived (or an await parked waiting for one).
    Signal,
    /// A child run was spawned; its name derives from this run's.
    Child,
    /// The run was asked to stop; propagates to children.
    Cancel,
    /// The function returned; the row holds the return value.
    Completed,
    /// A journaled ambient read (time, uuid): recorded on first
    /// execution, replayed identically forever.
    Ambient,
    /// A step attempt failed: the error, journaled. `final: true` means
    /// retries are exhausted and the run is parked failed until a
    /// resume.
    Failed,
}

impl EntryKind {
    /// The wire spelling, straight from the serde derive: one source of
    /// truth for both directions.
    fn as_str(self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_default()
    }

    fn parse(text: &str) -> Option<Self> {
        serde_json::from_value(serde_json::Value::String(text.to_owned())).ok()
    }
}

/// One journal row, as replay consumes it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Entry {
    pub seq: i64,
    /// Unix milliseconds the row was appended.
    pub at: i64,
    pub kind: EntryKind,
    pub data: serde_json::Value,
    pub format: i64,
}

/// Creates the journal table once per file; the version cell is the
/// record and carries the file forward when the schema moves.
///
/// # Errors
/// Returns SQLite's message.
pub fn ensure_schema(storage: &mut crate::storage::SqliteStorage) -> Result<(), String> {
    let version = storage.schema_version()?;
    if version >= SCHEMA_VERSION {
        return Ok(());
    }
    // The migration ladder: each arm carries one version forward, runs
    // exactly once per file (the version cell is the record), and rides
    // the call's transaction. New journal formats add arms here.
    if version == 0 {
        storage
            .platform()
            .execute(CREATE_JOURNAL, [])
            .map_err(|e| e.to_string())?;
    }
    if version <= 1 {
        storage
            .platform()
            .execute(CREATE_SECRET_PINS, [])
            .map_err(|e| e.to_string())?;
    }
    storage.set_schema_version(SCHEMA_VERSION)
}

/// Appends one entry and returns its sequence number. Appends ride the
/// current call's transaction like every platform write, so a journal
/// row commits exactly with the state it describes.
///
/// # Errors
/// Returns SQLite's message.
pub fn append(
    storage: &mut crate::storage::SqliteStorage,
    kind: EntryKind,
    data: &serde_json::Value,
) -> Result<i64, String> {
    let at = crate::extensions::objects::unix_now_ms();
    let connection = storage.platform();
    connection
        .execute(
            "INSERT INTO __actias_wf_journal (at, kind, data, format) VALUES (?, ?, ?, ?)",
            rusqlite::params![at, kind.as_str(), data.to_string(), ENTRY_FORMAT],
        )
        .map_err(|e| e.to_string())?;
    Ok(connection.last_insert_rowid())
}

/// Every entry at or after `from_seq`, in sequence order: the replay
/// read. `from_seq` of zero reads the whole journal.
///
/// # Errors
/// Returns SQLite's message; an unknown kind or undecodable data is an
/// error too, because replay must never silently skip history.
pub fn read_from(
    storage: &mut crate::storage::SqliteStorage,
    from_seq: i64,
) -> Result<Vec<Entry>, String> {
    let connection = storage.platform();
    let mut statement = connection
        .prepare(
            "SELECT seq, at, kind, data, format FROM __actias_wf_journal
             WHERE seq >= ? ORDER BY seq",
        )
        .map_err(|e| e.to_string())?;

    let rows = statement
        .query_map(rusqlite::params![from_seq], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut entries = Vec::new();
    for row in rows {
        let (seq, at, kind, data, format) = row.map_err(|e| e.to_string())?;
        let kind = EntryKind::parse(&kind)
            .ok_or_else(|| format!("journal entry {seq} has unknown kind '{kind}'"))?;
        let data = serde_json::from_str(&data)
            .map_err(|e| format!("journal entry {seq} does not decode: {e}"))?;
        entries.push(Entry {
            seq,
            at,
            kind,
            data,
            format,
        });
    }
    Ok(entries)
}

/// The newest entry, if any: what `status()` reads and what the console
/// lists instances by. Never a visibility store, just the head.
///
/// # Errors
/// Returns SQLite's message.
pub fn head(storage: &mut crate::storage::SqliteStorage) -> Result<Option<Entry>, String> {
    let mut entries = read_from_limit(storage, 1)?;
    Ok(entries.pop())
}

/// The newest `limit` entries, newest first; the head helper rides it.
fn read_from_limit(
    storage: &mut crate::storage::SqliteStorage,
    limit: i64,
) -> Result<Vec<Entry>, String> {
    let connection = storage.platform();
    let mut statement = connection
        .prepare(
            "SELECT seq, at, kind, data, format FROM __actias_wf_journal
             ORDER BY seq DESC LIMIT ?",
        )
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map(rusqlite::params![limit], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut entries = Vec::new();
    for row in rows {
        let (seq, at, kind, data, format) = row.map_err(|e| e.to_string())?;
        let kind = EntryKind::parse(&kind)
            .ok_or_else(|| format!("journal entry {seq} has unknown kind '{kind}'"))?;
        let data = serde_json::from_str(&data)
            .map_err(|e| format!("journal entry {seq} does not decode: {e}"))?;
        entries.push(Entry {
            seq,
            at,
            kind,
            data,
            format,
        });
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::SqliteStorage;

    fn open(dir: &tempfile::TempDir) -> SqliteStorage {
        let mut storage = SqliteStorage::open(&dir.path().join("wf.db")).expect("opens");
        ensure_schema(&mut storage).expect("schema");
        storage
    }

    /// A workflow vm over a real file, wired the way the worker will
    /// wire it: profile carries the cursor, app data carries the home.
    mod runs {
        use super::super::*;
        use crate::objects::{TaskOptions, spawn_object_task};
        use crate::proto::bundle::{Bundle, File};
        use crate::proto::script_service::{Revision, Script};
        use crate::runtime::{ActiasRuntime, PreparedRevision, VmProfile};
        use std::sync::Arc;

        const SOURCE: &str = r#"
            fn_runs = 0
            body_runs = 0

            workflow "greet" (function(wf, input)
                fn_runs = fn_runs + 1
                local id = uuid.v4()
                local charged = wf:step("charge", function()
                    body_runs = body_runs + 1
                    return { key = id, n = input.n, body_runs = body_runs }
                end)
                if fail_once and fn_runs == 1 then
                    error("transient outage after the step")
                end
                return { charged = charged, at = os.time(), fn_runs = fn_runs }
            end)

            on "fetch" (function()
                return { fn_runs = fn_runs, body_runs = body_runs }
            end)
        "#;

        async fn workflow_vm(source: &str, flag_fail_once: bool) -> (ActiasRuntime, Arc<WfShared>) {
            let channel =
                tonic::transport::Channel::from_static("http://127.0.0.1:1").connect_lazy();
            let shared = Arc::new(WfShared::default());
            let source = if flag_fail_once {
                format!("fail_once = true\n{source}")
            } else {
                source.to_owned()
            };
            let revision = Revision {
                bundle: Some(Bundle {
                    entry_point: "main.lua".to_owned(),
                    files: vec![File {
                        file_path: "main.lua".to_owned(),
                        content: source.into_bytes(),
                        ..Default::default()
                    }],
                }),
                ..Default::default()
            };
            let prepared =
                Arc::new(PreparedRevision::prepare(Script::default(), revision).expect("prepares"));
            let runtime = ActiasRuntime::with_profile(
                prepared,
                crate::proto::kv_service::kv_service_client::KvServiceClient::new(
                    crate::plain_grpc(channel),
                ),
                crate::egress::EgressClient::new(crate::egress::EgressPolicy::new([], false))
                    .expect("client builds"),
                None,
                None,
                None,
                VmProfile::Workflow {
                    source: shared.clone(),
                    secret_pins: None,
                },
            )
            .await
            .expect("workflow vm builds");
            runtime.set_app_data(shared.clone());
            (runtime, shared)
        }

        fn start_call(input: serde_json::Value) -> serde_json::Value {
            serde_json::json!({
                "class": actias_common::classes::WORKFLOW_CLASS,
                "name": "greet/run-1",
                "method": "start",
                "args": [input],
                "chain": ["p/__workflow/greet/run-1"],
            })
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn a_failed_attempt_replays_the_step_instead_of_rerunning_it() {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join("wf.db");

            let (runtime, _shared) = workflow_vm(SOURCE, true).await;
            let handle = spawn_object_task(
                runtime,
                TaskOptions {
                    storage: Some(crate::storage::SqliteStorage::open(&path).expect("opens")),
                    ..Default::default()
                },
            );

            // First attempt: the step runs, then the function fails past
            // it. The INTENT/RESULT pair survived their checkpoints.
            let failed = handle
                .call("__dispatch", start_call(serde_json::json!({ "n": 7 })))
                .await;
            assert!(failed.is_err(), "the transient outage surfaces");

            // Second attempt on the same instance: replay returns the
            // journaled step value; the body must not run again.
            let done = handle
                .call("__dispatch", start_call(serde_json::json!({ "n": 7 })))
                .await
                .expect("the retry completes");
            assert_eq!(done["status"], "completed");
            assert_eq!(done["value"]["charged"]["body_runs"], 1, "{done}");
            assert_eq!(done["value"]["charged"]["n"], 7);
            assert_eq!(done["value"]["fn_runs"], 2, "the function replayed");

            let counters = handle
                .call(
                    "__dispatch",
                    serde_json::json!({
                        "class": actias_common::classes::WORKFLOW_CLASS,
                        "name": "greet/run-1",
                        "method": "status",
                        "chain": ["p/__workflow/greet/run-1"],
                    }),
                )
                .await
                .expect("status answers");
            assert_eq!(counters["kind"], "COMPLETED");
        }

        const FAMILY_SOURCE: &str = r#"
            workflow "child" (function(wf, input)
                if input.slow then
                    wf:sleep("60s")
                end
                return { doubled = input.n * 2 }
            end)

            workflow "parent" (function(wf, input)
                local a = wf:spawn("child", { n = 1 })
                local b = wf:spawn("child", { n = 2 })
                -- The join is awaiting each child's completion signal;
                -- signals arrive in completion order, the scan matches.
                local rb = wf:await(b.signal)
                local ra = wf:await(a.signal)
                return { a = ra.value.doubled, b = rb.value.doubled }
            end)

            workflow "guardian" (function(wf, input)
                wf:spawn("child", { n = 1, slow = true })
                wf:await("never-comes")
                return {}
            end)

            workflow "gatherer" (function(wf, input)
                local a = wf:spawn("child", { n = 3 })
                local b = wf:spawn("child", { n = 5 })
                local results = wf:all { a, b }
                return { a = results[1].value.doubled, b = results[2].value.doubled }
            end)

            workflow "racer" (function(wf, input)
                local payload, winner = wf:race { "left", "right" }
                return { winner = winner, got = payload }
            end)
        "#;

        /// A tiny in-test placement: one workflow vm per identity, all
        /// sharing this router, so spawn, notify and cancel route for
        /// real without a worker.
        fn family_router(dir: std::path::PathBuf) -> crate::extensions::objects::ObjectRouter {
            use crate::extensions::objects::{ObjectRouter, ObjectTarget};
            type Registry =
                tokio::sync::Mutex<std::collections::HashMap<String, crate::objects::ObjectHandle>>;
            let registry: Arc<Registry> = Arc::default();
            let cell: Arc<std::sync::OnceLock<ObjectRouter>> = Arc::default();

            let registry_for = registry.clone();
            let cell_for = cell.clone();
            let dir_for = dir.clone();
            let router: ObjectRouter = Arc::new(move |target: ObjectTarget| {
                let registry = registry_for.clone();
                let cell = cell_for.clone();
                let dir = dir_for.clone();
                Box::pin(async move {
                    let key = format!("{}/{}", target.class, target.name);
                    let handle = {
                        let mut map = registry.lock().await;
                        if let Some(handle) = map.get(&key) {
                            handle.clone()
                        } else {
                            let (runtime, _shared) = workflow_vm(FAMILY_SOURCE, false).await;
                            let router = cell.get().expect("router installed").clone();
                            runtime.set_app_data::<ObjectRouter>(router);
                            let file =
                                dir.join(format!("{}.db", target.name.replace(['/', ':'], "_")));
                            let handle = spawn_object_task(
                                runtime,
                                TaskOptions {
                                    storage: Some(
                                        crate::storage::SqliteStorage::open(&file).expect("opens"),
                                    ),
                                    ..Default::default()
                                },
                            );
                            map.insert(key.clone(), handle.clone());
                            handle
                        }
                    };
                    handle
                        .call(
                            "__dispatch",
                            serde_json::json!({
                                "class": target.class,
                                "name": target.name,
                                "method": target.method,
                                "args": target.arguments,
                                "chain": [format!("p/{key}")],
                            }),
                        )
                        .await
                        .map_err(|e| e.to_string())
                })
            });
            cell.set(router.clone()).ok();
            router
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn spawned_children_complete_and_the_parent_joins_them() {
            let dir = tempfile::tempdir().expect("tempdir");
            let router = family_router(dir.path().to_path_buf());

            let started = router(crate::extensions::objects::ObjectTarget {
                class: actias_common::classes::WORKFLOW_CLASS.to_owned(),
                name: "parent/p1".to_owned(),
                method: "start".to_owned(),
                arguments: vec![serde_json::json!({})],
                chain: Vec::new(),
                caller: None,
            })
            .await
            .expect("parent starts");
            // The parent may park while the completion signals land.
            let _ = started;

            let mut done = serde_json::Value::Null;
            for _ in 0..60 {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                let joined = router(crate::extensions::objects::ObjectTarget {
                    class: actias_common::classes::WORKFLOW_CLASS.to_owned(),
                    name: "parent/p1".to_owned(),
                    method: "start".to_owned(),
                    arguments: vec![serde_json::json!({})],
                    chain: Vec::new(),
                    caller: None,
                })
                .await
                .expect("join answers");
                if joined["status"] == "completed" {
                    done = joined;
                    break;
                }
            }
            assert_eq!(done["status"], "completed", "parent never joined: {done}");
            assert_eq!(done["value"]["a"], 2, "{done}");
            assert_eq!(done["value"]["b"], 4);
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn all_returns_results_in_argument_order() {
            let dir = tempfile::tempdir().expect("tempdir");
            let router = family_router(dir.path().to_path_buf());

            router(crate::extensions::objects::ObjectTarget {
                class: actias_common::classes::WORKFLOW_CLASS.to_owned(),
                name: "gatherer/g1".to_owned(),
                method: "start".to_owned(),
                arguments: vec![serde_json::json!({})],
                chain: Vec::new(),
                caller: None,
            })
            .await
            .expect("gatherer starts");

            let mut done = serde_json::Value::Null;
            for _ in 0..60 {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                let joined = router(crate::extensions::objects::ObjectTarget {
                    class: actias_common::classes::WORKFLOW_CLASS.to_owned(),
                    name: "gatherer/g1".to_owned(),
                    method: "start".to_owned(),
                    arguments: vec![serde_json::json!({})],
                    chain: Vec::new(),
                    caller: None,
                })
                .await
                .expect("join answers");
                if joined["status"] == "completed" {
                    done = joined;
                    break;
                }
            }
            assert_eq!(done["status"], "completed", "gatherer never joined: {done}");
            assert_eq!(done["value"]["a"], 6, "{done}");
            assert_eq!(done["value"]["b"], 10);
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn race_returns_the_first_signal_and_its_name() {
            let dir = tempfile::tempdir().expect("tempdir");
            let router = family_router(dir.path().to_path_buf());

            let parked = router(crate::extensions::objects::ObjectTarget {
                class: actias_common::classes::WORKFLOW_CLASS.to_owned(),
                name: "racer/r1".to_owned(),
                method: "start".to_owned(),
                arguments: vec![serde_json::json!({})],
                chain: Vec::new(),
                caller: None,
            })
            .await
            .expect("racer starts");
            assert_eq!(parked["status"], "parked", "{parked}");

            // The SECOND listed name arrives first and wins.
            router(crate::extensions::objects::ObjectTarget {
                class: actias_common::classes::WORKFLOW_CLASS.to_owned(),
                name: "racer/r1".to_owned(),
                method: "signal".to_owned(),
                arguments: vec![
                    serde_json::json!("right"),
                    serde_json::json!({ "speed": "fast" }),
                ],
                chain: Vec::new(),
                caller: None,
            })
            .await
            .expect("signal lands");

            let mut done = serde_json::Value::Null;
            for _ in 0..60 {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                let joined = router(crate::extensions::objects::ObjectTarget {
                    class: actias_common::classes::WORKFLOW_CLASS.to_owned(),
                    name: "racer/r1".to_owned(),
                    method: "start".to_owned(),
                    arguments: vec![serde_json::json!({})],
                    chain: Vec::new(),
                    caller: None,
                })
                .await
                .expect("join answers");
                if joined["status"] == "completed" {
                    done = joined;
                    break;
                }
            }
            assert_eq!(done["status"], "completed", "racer never finished: {done}");
            assert_eq!(done["value"]["winner"], "right", "{done}");
            assert_eq!(done["value"]["got"]["speed"], "fast");
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn cancelling_the_parent_reaches_its_children() {
            let dir = tempfile::tempdir().expect("tempdir");
            let router = family_router(dir.path().to_path_buf());

            let call_target = |method: &str, name: &str| crate::extensions::objects::ObjectTarget {
                class: actias_common::classes::WORKFLOW_CLASS.to_owned(),
                name: name.to_owned(),
                method: method.to_owned(),
                arguments: vec![serde_json::json!({})],
                chain: Vec::new(),
                caller: None,
            };
            let parked = router(call_target("start", "guardian/g1"))
                .await
                .expect("parent parks");
            assert_eq!(parked["status"], "parked", "{parked}");

            let cancelled = router(call_target("cancel", "guardian/g1"))
                .await
                .expect("cancel answers");
            assert_eq!(cancelled["status"], "cancelled");

            // The child was spawned with a 60s sleep; only propagation
            // can end it this decade.
            let child_status = router(call_target("status", "child/g1.0"))
                .await
                .expect("child status answers");
            assert_eq!(child_status["kind"], "CANCEL", "{child_status}");
        }

        const PARKING_SOURCE: &str = r#"
            workflow "waity" (function(wf, input)
                if input.mode == "sleep" then
                    wf:sleep("60ms")
                    return { woke = true, at = os.time() }
                end
                local approval = wf:await("approval", { timeout = "24h" })
                if approval == nil then
                    return { status = "timed-out" }
                end
                return { status = "approved", by = approval.by }
            end)
        "#;

        fn call(name: &str, method: &str, args: serde_json::Value) -> serde_json::Value {
            serde_json::json!({
                "class": actias_common::classes::WORKFLOW_CLASS,
                "name": name,
                "method": method,
                "args": args,
                "chain": [format!("p/__workflow/{name}")],
            })
        }

        const RETRY_SOURCE: &str = r#"
            tries = 0
            workflow "flaky" (function(wf, input)
                local report = wf:step("run-tests", {
                    retries = 3,
                    backoff = "40ms",
                }, function()
                    tries = tries + 1
                    if tries < 3 then
                        error("sandbox timeout")
                    end
                    return { passed = true, on_attempt = tries }
                end)
                return report
            end)

            workflow "doomed" (function(wf, input)
                local report = wf:step("run-tests", {
                    retries = 2,
                    backoff = "30ms",
                }, function()
                    tries = tries + 1
                    if tries <= 2 then
                        error("sandbox timeout")
                    end
                    return { passed = true, on_attempt = tries }
                end)
                return report
            end)

            local jobs = queue "jobs"
            workflow "leaky" (function(wf, input)
                jobs:send({ n = 1 })
                return {}
            end)
        "#;

        async fn status_until(
            handle: &crate::objects::ObjectHandle,
            name: &str,
            wanted: &str,
        ) -> serde_json::Value {
            for _ in 0..60 {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                let status = handle
                    .call("__dispatch", call(name, "status", serde_json::json!([])))
                    .await
                    .expect("status answers");
                if status["kind"] == wanted {
                    return status;
                }
            }
            serde_json::Value::Null
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn retries_park_with_backoff_and_converge() {
            let dir = tempfile::tempdir().expect("tempdir");
            let (runtime, _shared) = workflow_vm(RETRY_SOURCE, false).await;
            let handle = spawn_object_task(
                runtime,
                TaskOptions {
                    storage: Some(
                        crate::storage::SqliteStorage::open(&dir.path().join("wf.db"))
                            .expect("opens"),
                    ),
                    ..Default::default()
                },
            );

            let first = handle
                .call(
                    "__dispatch",
                    call("flaky/r1", "start", serde_json::json!([{}])),
                )
                .await
                .expect("first attempt parks for its retry");
            assert_eq!(first["status"], "parked", "{first}");

            let done = status_until(&handle, "flaky/r1", "COMPLETED").await;
            assert_eq!(done["kind"], "COMPLETED", "retries never converged");

            let joined = handle
                .call(
                    "__dispatch",
                    call("flaky/r1", "start", serde_json::json!([{}])),
                )
                .await
                .expect("joins");
            assert_eq!(joined["value"]["on_attempt"], 3, "{joined}");
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn exhausted_retries_fail_the_run_and_resume_reenters() {
            let dir = tempfile::tempdir().expect("tempdir");
            let (runtime, _shared) = workflow_vm(RETRY_SOURCE, false).await;
            let handle = spawn_object_task(
                runtime,
                TaskOptions {
                    storage: Some(
                        crate::storage::SqliteStorage::open(&dir.path().join("wf.db"))
                            .expect("opens"),
                    ),
                    ..Default::default()
                },
            );

            handle
                .call(
                    "__dispatch",
                    call("doomed/r1", "start", serde_json::json!([{}])),
                )
                .await
                .expect("parks for retry");
            // Attempt two fails on its own alarm; the run lands failed.
            let mut failed = serde_json::Value::Null;
            for _ in 0..60 {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                let joined = handle
                    .call(
                        "__dispatch",
                        call("doomed/r1", "start", serde_json::json!([{}])),
                    )
                    .await
                    .expect("join answers");
                if joined["status"] == "failed" {
                    failed = joined;
                    break;
                }
            }
            assert_eq!(failed["status"], "failed", "never failed: {failed}");

            // Resume: fresh attempts at the failed step; the third body
            // run succeeds, everything before it replays untouched.
            let resumed = handle
                .call(
                    "__dispatch",
                    call("doomed/r1", "resume", serde_json::json!([])),
                )
                .await
                .expect("resume answers");
            assert_eq!(resumed["status"], "completed", "{resumed}");
            assert_eq!(resumed["value"]["on_attempt"], 3);
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn effects_outside_steps_are_refused() {
            let dir = tempfile::tempdir().expect("tempdir");
            let (runtime, _shared) = workflow_vm(RETRY_SOURCE, false).await;
            let handle = spawn_object_task(
                runtime,
                TaskOptions {
                    storage: Some(
                        crate::storage::SqliteStorage::open(&dir.path().join("wf.db"))
                            .expect("opens"),
                    ),
                    ..Default::default()
                },
            );

            let refused = handle
                .call(
                    "__dispatch",
                    call("leaky/r1", "start", serde_json::json!([{}])),
                )
                .await;
            let text = format!("{:#}", refused.expect_err("must refuse"));
            assert!(
                text.contains(crate::extensions::determinism::FORBIDDEN),
                "wrong refusal: {text}"
            );
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn a_sleep_parks_and_the_alarm_wakes_it_to_completion() {
            let dir = tempfile::tempdir().expect("tempdir");
            let (runtime, _shared) = workflow_vm(PARKING_SOURCE, false).await;
            let handle = spawn_object_task(
                runtime,
                TaskOptions {
                    storage: Some(
                        crate::storage::SqliteStorage::open(&dir.path().join("wf.db"))
                            .expect("opens"),
                    ),
                    ..Default::default()
                },
            );

            let parked = handle
                .call(
                    "__dispatch",
                    call(
                        "waity/nap-1",
                        "start",
                        serde_json::json!([{ "mode": "sleep" }]),
                    ),
                )
                .await
                .expect("parks");
            assert_eq!(parked["status"], "parked", "{parked}");

            // The pinned task's own alarm loop fires the wake; nothing
            // here touches the object again.
            let mut woke = serde_json::Value::Null;
            for _ in 0..40 {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                let status = handle
                    .call(
                        "__dispatch",
                        call("waity/nap-1", "status", serde_json::json!([])),
                    )
                    .await
                    .expect("status answers");
                if status["kind"] == "COMPLETED" {
                    woke = status;
                    break;
                }
            }
            assert_eq!(woke["kind"], "COMPLETED", "the alarm never woke the run");

            let joined = handle
                .call(
                    "__dispatch",
                    call(
                        "waity/nap-1",
                        "start",
                        serde_json::json!([{ "mode": "sleep" }]),
                    ),
                )
                .await
                .expect("joins");
            assert_eq!(joined["value"]["woke"], true, "{joined}");
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn a_signal_wakes_a_parked_await() {
            let dir = tempfile::tempdir().expect("tempdir");
            let (runtime, _shared) = workflow_vm(PARKING_SOURCE, false).await;
            let handle = spawn_object_task(
                runtime,
                TaskOptions {
                    storage: Some(
                        crate::storage::SqliteStorage::open(&dir.path().join("wf.db"))
                            .expect("opens"),
                    ),
                    ..Default::default()
                },
            );

            let parked = handle
                .call(
                    "__dispatch",
                    call(
                        "waity/gate-1",
                        "start",
                        serde_json::json!([{ "mode": "await" }]),
                    ),
                )
                .await
                .expect("parks");
            assert_eq!(parked["status"], "parked", "{parked}");

            let resumed = handle
                .call(
                    "__dispatch",
                    call(
                        "waity/gate-1",
                        "signal",
                        serde_json::json!(["approval", { "by": "jsh32" }]),
                    ),
                )
                .await
                .expect("the signal resumes the run");
            assert_eq!(resumed["status"], "completed", "{resumed}");
            assert_eq!(resumed["value"]["by"], "jsh32");
        }

        /// The double-clicked button: a signal sent twice journals
        /// twice, one row goes unconsumed, and replay must set it
        /// aside instead of wedging the run at the next verb (found
        /// live: "journal divergence: expected Signal, code reached
        /// step 'settle'").
        #[tokio::test(flavor = "multi_thread")]
        async fn a_duplicate_signal_never_wedges_replay() {
            const SOURCE: &str = r#"
                workflow "handover" (function(wf, input)
                    local a = wf:await("shipped")
                    local b = wf:await("received")
                    local sealed = wf:step("settle", function()
                        return { done = true }
                    end)
                    return { a = a.n, b = b.n, done = sealed.done }
                end)
                on "fetch" (function() return { body = "ok" } end)
            "#;

            let dir = tempfile::tempdir().expect("tempdir");
            let (runtime, _shared) = workflow_vm(SOURCE, false).await;
            let handle = spawn_object_task(
                runtime,
                TaskOptions {
                    storage: Some(
                        crate::storage::SqliteStorage::open(&dir.path().join("wf.db"))
                            .expect("opens"),
                    ),
                    ..Default::default()
                },
            );

            let parked = handle
                .call(
                    "__dispatch",
                    call("handover/sale-1", "start", serde_json::json!([{}])),
                )
                .await
                .expect("parks at shipped");
            assert_eq!(parked["status"], "parked", "{parked}");

            let first = handle
                .call(
                    "__dispatch",
                    call(
                        "handover/sale-1",
                        "signal",
                        serde_json::json!(["shipped", { "n": 1 }]),
                    ),
                )
                .await
                .expect("first shipped consumed");
            assert_eq!(first["status"], "parked", "now at received: {first}");

            // The double click: a second `shipped` nobody will consume.
            let duplicate = handle
                .call(
                    "__dispatch",
                    call(
                        "handover/sale-1",
                        "signal",
                        serde_json::json!(["shipped", { "n": 2 }]),
                    ),
                )
                .await
                .expect("a duplicate signal must not error the run");
            assert_eq!(
                duplicate["status"], "parked",
                "still parked at received, not wedged: {duplicate}"
            );

            let done = handle
                .call(
                    "__dispatch",
                    call(
                        "handover/sale-1",
                        "signal",
                        serde_json::json!(["received", { "n": 3 }]),
                    ),
                )
                .await
                .expect("received completes the run");
            assert_eq!(done["status"], "completed", "{done}");
            assert_eq!(done["value"]["a"], 1, "the FIRST shipped won: {done}");
            assert_eq!(done["value"]["b"], 3);
            assert_eq!(done["value"]["done"], true);
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn cancel_wins_over_further_progress() {
            let dir = tempfile::tempdir().expect("tempdir");
            let (runtime, _shared) = workflow_vm(PARKING_SOURCE, false).await;
            let handle = spawn_object_task(
                runtime,
                TaskOptions {
                    storage: Some(
                        crate::storage::SqliteStorage::open(&dir.path().join("wf.db"))
                            .expect("opens"),
                    ),
                    ..Default::default()
                },
            );

            handle
                .call(
                    "__dispatch",
                    call(
                        "waity/gone-1",
                        "start",
                        serde_json::json!([{ "mode": "await" }]),
                    ),
                )
                .await
                .expect("parks");
            let cancelled = handle
                .call(
                    "__dispatch",
                    call(
                        "waity/gone-1",
                        "cancel",
                        serde_json::json!(["customer withdrew"]),
                    ),
                )
                .await
                .expect("cancels");
            assert_eq!(cancelled["status"], "cancelled");

            // A late signal does not resurrect the run.
            let after = handle
                .call(
                    "__dispatch",
                    call(
                        "waity/gone-1",
                        "signal",
                        serde_json::json!(["approval", { "by": "too-late" }]),
                    ),
                )
                .await
                .expect("answers");
            assert_eq!(after["status"], "cancelled", "{after}");
            assert_eq!(after["reason"], "customer withdrew");
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn joining_a_completed_run_returns_the_recorded_outcome() {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join("wf.db");

            let (runtime, _shared) = workflow_vm(SOURCE, false).await;
            let handle = spawn_object_task(
                runtime,
                TaskOptions {
                    storage: Some(crate::storage::SqliteStorage::open(&path).expect("opens")),
                    ..Default::default()
                },
            );

            let first = handle
                .call("__dispatch", start_call(serde_json::json!({ "n": 1 })))
                .await
                .expect("completes");

            // A retried start joins the finished run: same recorded value,
            // nothing executes again.
            let joined = handle
                .call("__dispatch", start_call(serde_json::json!({ "n": 999 })))
                .await
                .expect("joins");
            assert_eq!(first, joined);

            let probe = handle.call("fetch", serde_json::Value::Null).await;
            // The fetch listener is a user-class shape this platform
            // object does not dispatch; the counters are already proven
            // via the returned value.
            let _ = probe;
            assert_eq!(first["value"]["charged"]["body_runs"], 1);
        }
    }

    #[test]
    fn appends_read_back_in_order_across_reopen() {
        let dir = tempfile::tempdir().expect("tempdir");
        {
            let mut storage = open(&dir);
            append(
                &mut storage,
                EntryKind::Started,
                &serde_json::json!({ "revision": "rev-1", "seed": 42 }),
            )
            .expect("appends");
            append(
                &mut storage,
                EntryKind::Intent,
                &serde_json::json!({ "step": "charge-card" }),
            )
            .expect("appends");
        }

        // A new open over the same file: the journal IS the durability.
        let mut storage = open(&dir);
        let seq = append(
            &mut storage,
            EntryKind::Result,
            &serde_json::json!({ "step": "charge-card", "value": { "ok": true } }),
        )
        .expect("appends");
        assert_eq!(seq, 3, "sequence continues across reopen");

        let entries = read_from(&mut storage, 0).expect("reads");
        assert_eq!(
            entries.iter().map(|e| e.kind).collect::<Vec<_>>(),
            vec![EntryKind::Started, EntryKind::Intent, EntryKind::Result],
        );
        assert_eq!(entries[0].data["seed"], 42);
        assert!(entries.iter().all(|e| e.format == ENTRY_FORMAT));

        // The cursor read: replay resumes past what it already consumed.
        let tail = read_from(&mut storage, 2).expect("reads");
        assert_eq!(tail.len(), 2);
        assert_eq!(tail[0].kind, EntryKind::Intent);
    }

    #[test]
    fn the_head_is_the_newest_entry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut storage = open(&dir);
        assert_eq!(head(&mut storage).expect("reads"), None);

        append(&mut storage, EntryKind::Started, &serde_json::json!({})).expect("appends");
        append(
            &mut storage,
            EntryKind::Completed,
            &serde_json::json!({ "value": "fulfilled" }),
        )
        .expect("appends");

        let newest = head(&mut storage).expect("reads").expect("has entries");
        assert_eq!(newest.kind, EntryKind::Completed);
        assert_eq!(newest.seq, 2);
    }

    #[test]
    fn secret_pins_round_trip_and_survive_reload() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("wf.db");

        let pins = SecretPins::load(&file).expect("loads fresh");
        assert_eq!(pins.version_for("stripe-live"), None);
        pins.record("stripe-live", 3).expect("records");
        assert_eq!(pins.version_for("stripe-live"), Some(3));

        // A later vm build loads the same pins from the file; a second
        // record of the same name never moves the pin.
        let pins = SecretPins::load(&file).expect("reloads");
        assert_eq!(pins.version_for("stripe-live"), Some(3));
        pins.record("stripe-live", 9).expect("ignored");
        let pins = SecretPins::load(&file).expect("reloads again");
        assert_eq!(pins.version_for("stripe-live"), Some(3));
    }

    #[test]
    fn a_version_one_file_gains_the_pin_table_through_the_ladder() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("wf.db");

        // A file the previous format wrote: journal table, version 1.
        {
            let mut storage = crate::storage::SqliteStorage::open(&file).expect("opens");
            storage
                .platform()
                .execute(CREATE_JOURNAL, [])
                .expect("creates journal");
            storage.set_schema_version(1).expect("stamps v1");
        }

        let pins = SecretPins::load(&file).expect("ladder upgrades");
        pins.record("api-token", 1).expect("records");

        let mut storage = crate::storage::SqliteStorage::open(&file).expect("reopens");
        assert_eq!(storage.schema_version().expect("reads"), SCHEMA_VERSION);
        assert!(
            storage
                .table_exists("__actias_wf_journal")
                .expect("journal check"),
            "the journal survives the upgrade"
        );
    }

    #[test]
    fn ensure_schema_is_idempotent_and_versioned() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut storage = open(&dir);
        // A second ensure over a current file is a no-op, not an error.
        ensure_schema(&mut storage).expect("idempotent");
        assert_eq!(storage.schema_version().expect("reads"), SCHEMA_VERSION);
    }
}

/// The revision id STARTED pinned, when the file already holds a run:
/// what the spawn factory replays instead of the owner's current
/// revision, the one deliberate exception to always-current. A fresh or
/// unreadable file reads as unpinned; journal problems surface at
/// dispatch, not here.
pub fn pinned_revision(file: &std::path::Path) -> Option<String> {
    let mut storage = crate::storage::SqliteStorage::open_read_only(file).ok()?;
    if !storage.table_exists("__actias_wf_journal").ok()? {
        return None;
    }
    let first = read_from(&mut storage, 0).ok()?.into_iter().next()?;
    if first.kind != EntryKind::Started {
        return None;
    }
    first.data["revision"].as_str().map(str::to_owned)
}

/// The run's secret pins, loaded from its journal file before the vm
/// builds (which is why the file opens before the task does): the map
/// answers declarations synchronously, and a first resolution persists
/// its pin before the value is ever used.
pub struct SecretPins {
    file: std::path::PathBuf,
    known: std::sync::Mutex<std::collections::HashMap<String, u64>>,
}

impl SecretPins {
    /// Opens the instance file (creating a fresh one when the run has
    /// never lived here) and loads every pin it holds.
    ///
    /// # Errors
    /// Returns SQLite's message.
    pub fn load(file: &std::path::Path) -> Result<Self, String> {
        let mut storage = crate::storage::SqliteStorage::open(file)?;
        ensure_schema(&mut storage)?;

        let mut known = std::collections::HashMap::new();
        let connection = storage.platform();
        let mut statement = connection
            .prepare("SELECT name, version FROM __actias_wf_secrets")
            .map_err(|e| e.to_string())?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|e| e.to_string())?;
        for row in rows {
            let (name, version) = row.map_err(|e| e.to_string())?;
            known.insert(name, version as u64);
        }

        Ok(SecretPins {
            file: file.to_owned(),
            known: std::sync::Mutex::new(known),
        })
    }

    /// The pinned version, when the run has resolved this name before.
    pub fn version_for(&self, name: &str) -> Option<u64> {
        self.known
            .lock()
            .expect("no poisoned lock")
            .get(name)
            .copied()
    }

    /// Persists a first resolution's pin before its value is used; a pin
    /// that cannot persist fails the declaration, because a replay that
    /// resolved differently would diverge.
    ///
    /// # Errors
    /// Returns SQLite's message.
    pub fn record(&self, name: &str, version: u64) -> Result<(), String> {
        let mut storage = crate::storage::SqliteStorage::open(&self.file)?;
        storage
            .platform()
            .execute(
                "INSERT OR IGNORE INTO __actias_wf_secrets (name, version) VALUES (?, ?)",
                rusqlite::params![name, version as i64],
            )
            .map_err(|e| e.to_string())?;
        self.known
            .lock()
            .expect("no poisoned lock")
            .insert(name.to_owned(), version);
        Ok(())
    }
}

/// Refuses effects outside step bodies in workflow vms; a no-op in
/// every other vm. The gate teaching text is the determinism module's.
pub fn assert_effects_allowed(lua: &mlua::Lua) -> mlua::Result<()> {
    if let Some(shared) = lua.app_data_ref::<std::sync::Arc<WfShared>>()
        && !shared.effects_allowed()
    {
        return Err(mlua::Error::RuntimeError(
            crate::extensions::determinism::FORBIDDEN.to_owned(),
        ));
    }
    Ok(())
}

/// The journal off a read-only connection: a file that never held a
/// journal reads as empty rather than erroring, so dashboards can probe
/// any workflow identity.
pub fn read_journal_readonly(
    storage: &mut crate::storage::SqliteStorage,
) -> Result<Vec<Entry>, String> {
    read_journal_readonly_from(storage, 0)
}

/// Like [`read_journal_readonly`], from a cursor.
pub fn read_journal_readonly_from(
    storage: &mut crate::storage::SqliteStorage,
    since: i64,
) -> Result<Vec<Entry>, String> {
    if !storage.table_exists("__actias_wf_journal")? {
        return Ok(Vec::new());
    }
    read_from(storage, since)
}

/// The status a journal tells on its own: replay determinism means the
/// tail IS the state. Terminal kinds win; a trailing timer is a park
/// (sleeping, or awaiting unless its signal already arrived); anything
/// else reads as running.
/// The step label a runs table shows: the dangling intent, the gate, or
/// the last completed step.
pub fn at_step(entries: &[Entry]) -> serde_json::Value {
    let mut last_result: Option<&str> = None;
    let mut dangling: Option<&str> = None;
    for entry in entries {
        match entry.kind {
            EntryKind::Intent => dangling = entry.data["step"].as_str(),
            EntryKind::Result => {
                dangling = None;
                last_result = entry.data["step"].as_str();
            }
            _ => {}
        }
    }
    if let Some(step) = dangling {
        return serde_json::json!(step);
    }
    match entries.last() {
        Some(last) if last.kind == EntryKind::Timer => {
            let gate = &last.data["for"];
            if gate.is_null() {
                serde_json::json!("sleep")
            } else if let Some(set) = gate.as_array() {
                serde_json::json!(format!(
                    "await {}",
                    set.iter()
                        .filter_map(|name| name.as_str())
                        .collect::<Vec<_>>()
                        .join(" | ")
                ))
            } else {
                serde_json::json!(format!("await {}", gate.as_str().unwrap_or("?")))
            }
        }
        Some(last) if last.kind == EntryKind::Failed => {
            serde_json::json!(last.data["step"].as_str().unwrap_or("failed"))
        }
        Some(last) if last.kind == EntryKind::Completed => serde_json::json!("done"),
        Some(last) if last.kind == EntryKind::Cancel => serde_json::json!("cancelled"),
        _ => last_result
            .map(|step| serde_json::json!(format!("after {step}")))
            .unwrap_or(serde_json::json!("start")),
    }
}

pub fn run_status(entries: &[Entry]) -> serde_json::Value {
    if let Some(cancelled) = entries.iter().find(|e| e.kind == EntryKind::Cancel) {
        return serde_json::json!({
            "status": "cancelled",
            "reason": cancelled.data["reason"],
            "at": cancelled.at,
        });
    }
    if let Some(done) = entries.iter().find(|e| e.kind == EntryKind::Completed) {
        return serde_json::json!({ "status": "completed", "at": done.at });
    }
    let started = entries.first().map(|e| e.at);
    match entries.last() {
        None => serde_json::json!({ "status": "unstarted" }),
        Some(last)
            if last.kind == EntryKind::Failed && last.data["final"].as_bool().unwrap_or(false) =>
        {
            serde_json::json!({
                "status": "failed",
                "step": last.data["step"],
                "error": last.data["error"],
                "attempts": last.data["attempt"],
                "started_at": started,
            })
        }
        Some(last) if last.kind == EntryKind::Timer => {
            let gate = &last.data["for"];
            if gate.is_null() {
                serde_json::json!({
                    "status": "sleeping",
                    "due_ms": last.data["due_ms"],
                    "started_at": started,
                })
            } else {
                serde_json::json!({
                    "status": "awaiting",
                    "signal": gate,
                    "due_ms": last.data["due_ms"],
                    "started_at": started,
                })
            }
        }
        Some(_) => serde_json::json!({ "status": "running", "started_at": started }),
    }
}

/// One run-attempt's replay state: the journal tail not yet consumed,
/// and the instance's deterministic generator. Live mode is simply the
/// tail running out.
struct Attempt {
    pending: std::collections::VecDeque<Entry>,
    home: std::sync::Arc<crate::objects::ObjectHome>,
    rng: u64,
    /// The instance's own key and name, for arming its alarm from verbs.
    own_key: String,
    name: String,
    /// True for exactly one attempt after a resume dispatch: the step
    /// whose final failure blocks the run consumes it and retries.
    resume: bool,
}

impl Attempt {
    /// One uniform draw; xorshift64*, engine-independent, stepped
    /// identically on replay because the seed and the draw order are.
    fn draw(&mut self) -> f64 {
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        (self.rng.wrapping_mul(0x2545F4914F6CDD1D) >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// The per-instance cell the vm profile's shims and the dispatch share:
/// the determinism source IS the replay cursor.
#[derive(Default)]
pub struct WfShared {
    attempt: std::sync::Mutex<Option<Attempt>>,
    /// Set by a verb just before it unwinds the run to park it; the
    /// attempt runner reads it back to tell a park from a failure, so
    /// nothing ever sniffs error strings.
    parked: std::sync::Mutex<Option<String>>,
    /// Set when a step exhausts its retries: the run is failed, not
    /// broken; a resume re-enters at that step.
    failed: std::sync::Mutex<Option<String>>,
    /// True while a step body executes: the one window where effects
    /// (kv, objects, http-in-context) are allowed in a workflow vm.
    in_step: std::sync::atomic::AtomicBool,
    /// Step results substituted by the test harness: a faked step never
    /// runs its body but journals exactly like a real one, so replay
    /// cannot tell tests from production.
    fakes: std::sync::Mutex<std::collections::HashMap<String, serde_json::Value>>,
}

impl WfShared {
    /// Parks the run: records why, then unwinds the Lua stack. The
    /// error is only the vehicle; the flag is the truth.
    fn park(&self, reason: String) -> mlua::Error {
        *self.parked.lock().expect("no poisoned lock") = Some(reason);
        mlua::Error::RuntimeError("workflow parked".to_owned())
    }

    /// Fails the run: retries are exhausted, the journal holds the
    /// final error, and only a resume re-enters the step.
    fn fail(&self, reason: String) -> mlua::Error {
        *self.failed.lock().expect("no poisoned lock") = Some(reason);
        mlua::Error::RuntimeError("workflow failed".to_owned())
    }

    /// Whether a step body is executing right now: the effect window.
    pub fn effects_allowed(&self) -> bool {
        self.in_step.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Installs test fakes: step name to the value its body would have
    /// returned.
    pub fn set_fakes(&self, fakes: std::collections::HashMap<String, serde_json::Value>) {
        *self.fakes.lock().expect("no poisoned lock") = fakes;
    }
}

/// Rotates signals nobody has consumed yet from the replay tail's
/// front to its back. Signals journal on ARRIVAL (a double-clicked
/// button, an early child completion), awaits consume them BY NAME
/// from anywhere in the tail, and every other verb replays effects in
/// strict order; without this, one unconsumed duplicate wedges the run
/// at the next verb ("journal divergence: expected Signal"), which a
/// double-clicked ship button produced live. Rotation is deterministic
/// per replay, so the journal's guarantees hold.
fn set_aside_leading_signals(pending: &mut std::collections::VecDeque<Entry>) {
    let mut budget = pending.len();
    while budget > 0
        && pending
            .front()
            .is_some_and(|entry| entry.kind == EntryKind::Signal)
    {
        let entry = pending.pop_front().expect("front checked");
        pending.push_back(entry);
        budget -= 1;
    }
}

impl WfShared {
    /// One journaled ambient read: replayed from the cursor when the
    /// tail still holds one, appended live otherwise.
    fn ambient(
        &self,
        tag: &str,
        live: impl FnOnce() -> serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let mut guard = self.attempt.lock().expect("no poisoned lock");
        let attempt = guard
            .as_mut()
            .ok_or_else(|| "No workflow attempt is executing.".to_owned())?;

        set_aside_leading_signals(&mut attempt.pending);
        if let Some(entry) = attempt
            .pending
            .front()
            .filter(|entry| entry.kind != EntryKind::Signal)
        {
            if entry.kind != EntryKind::Ambient || entry.data["tag"] != tag {
                return Err(format!(
                    "journal divergence: expected {:?} '{}', code asked for ambient '{tag}'",
                    entry.kind, entry.data["tag"]
                ));
            }
            let value = entry.data["value"].clone();
            attempt.pending.pop_front();
            return Ok(value);
        }

        let value = live();
        let record = serde_json::json!({ "tag": tag, "value": value });
        attempt
            .home
            .with_storage(|storage| append(storage, EntryKind::Ambient, &record))?;
        Ok(value)
    }
}

impl crate::extensions::determinism::Determinism for WfShared {
    fn time(&self) -> i64 {
        self.ambient("time", || {
            serde_json::json!(crate::extensions::objects::unix_now_ms() / 1000)
        })
        .ok()
        .and_then(|value| value.as_i64())
        .unwrap_or(0)
    }

    fn uuid(&self) -> String {
        self.ambient("uuid", || {
            serde_json::json!(uuid::Uuid::new_v4().to_string())
        })
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
    }

    fn random(&self) -> f64 {
        let mut guard = self.attempt.lock().expect("no poisoned lock");
        guard.as_mut().map(Attempt::draw).unwrap_or(0.0)
    }
}

/// The timeout option the waiting verbs accept: a duration string or
/// whole seconds; absent waits forever.
fn parse_timeout(opts: Option<mlua::Table>) -> mlua::Result<Option<i64>> {
    opts.and_then(|table| table.get::<mlua::Value>("timeout").ok())
        .map(|value| match value {
            mlua::Value::String(raw) => {
                crate::extensions::objects::parse_duration_ms(&raw.to_str()?)
                    .map_err(mlua::Error::RuntimeError)
            }
            mlua::Value::Integer(seconds) => Ok(seconds * 1000),
            mlua::Value::Nil => Ok(i64::MAX),
            _ => Err(mlua::Error::RuntimeError(
                "the timeout is a duration.".to_owned(),
            )),
        })
        .transpose()
}

/// The signal each entry of `wf:all`/`wf:race` waits on: a spawned
/// job's table (its `signal` field) or a bare signal name.
fn signal_names(jobs: &mlua::Table) -> mlua::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in jobs.clone().sequence_values::<mlua::Value>() {
        match entry? {
            mlua::Value::String(name) => names.push(name.to_str()?.to_owned()),
            mlua::Value::Table(job) => names.push(job.get::<String>("signal").map_err(|_| {
                mlua::Error::RuntimeError(
                    "each entry is a spawned job or a signal name.".to_owned(),
                )
            })?),
            _ => {
                return Err(mlua::Error::RuntimeError(
                    "each entry is a spawned job or a signal name.".to_owned(),
                ));
            }
        }
    }
    Ok(names)
}

/// One gate over a set of signals: `await` is the one-name form, `race`
/// the many. The gate row keeps the bare-string shape for a single name
/// (the shape every existing journal holds) and an array for several.
/// The answer is the FIRST matching signal after the gate in journal
/// order, which is completion order, which is deterministic; the
/// returned name says which one it was. A timeout returns (nil, None).
fn await_signals(
    lua: &mlua::Lua,
    names: &[String],
    timeout_ms: Option<i64>,
) -> mlua::Result<(mlua::Value, Option<String>)> {
    use mlua::LuaSerdeExt;

    let gate_json = if names.len() == 1 {
        serde_json::json!(names[0])
    } else {
        serde_json::json!(names)
    };
    let wanted = |entry: &Entry| {
        entry.kind == EntryKind::Signal
            && names.iter().any(|name| entry.data["name"] == name.as_str())
    };
    let describe = names.join("', '");

    let shared = lua
        .app_data_ref::<std::sync::Arc<WfShared>>()
        .map(|shared| shared.clone())
        .ok_or_else(|| mlua::Error::RuntimeError("Not a workflow vm.".to_owned()))?;
    let mut guard = shared.attempt.lock().expect("no poisoned lock");
    let attempt = guard
        .as_mut()
        .ok_or_else(|| mlua::Error::RuntimeError("No workflow attempt is executing.".to_owned()))?;
    let now = crate::extensions::objects::unix_now_ms();

    // The gate row: live appends it; replay finds it first. A gate that
    // never journaled (the run parked at an earlier verb) may instead
    // meet its signal directly: consume it and never park at all.
    // Deterministic, since the journal is. Unconsumed foreign signals
    // (a double-sent button, an unawaited name) rotate aside first and
    // never block the gate.
    set_aside_leading_signals(&mut attempt.pending);
    let front_is_gate = attempt
        .pending
        .front()
        .is_some_and(|entry| entry.kind == EntryKind::Timer && entry.data["for"] == gate_json);
    if !front_is_gate {
        if attempt.pending.iter().any(wanted) {
            let offset = attempt
                .pending
                .iter()
                .position(wanted)
                .expect("checked above");
            let signal = attempt
                .pending
                .remove(offset)
                .expect("position came from this deque");
            let winner = signal.data["name"].as_str().map(str::to_owned);
            return Ok((lua.to_value(&signal.data["payload"])?, winner));
        }
        if let Some(entry) = attempt
            .pending
            .front()
            .filter(|entry| entry.kind != EntryKind::Signal)
        {
            return Err(mlua::Error::RuntimeError(format!(
                "journal divergence: expected {:?}, code reached await '{describe}'",
                entry.kind
            )));
        }
        // Tail exhausted (foreign signals alone do not count): journal
        // the gate and park.
        let due = timeout_ms.map(|ms| now.saturating_add(ms.max(0)));
        attempt
            .home
            .with_storage(|storage| {
                append(
                    storage,
                    EntryKind::Timer,
                    &serde_json::json!({ "due_ms": due, "for": gate_json }),
                )?;
                storage.commit()?;
                storage.begin()
            })
            .map_err(mlua::Error::RuntimeError)?;
        if let Some(ms) = timeout_ms.filter(|ms| *ms < i64::MAX) {
            arm(attempt, ms.max(0)).map_err(mlua::Error::RuntimeError)?;
        }
        drop(guard);
        return Err(shared.park(format!("awaiting '{describe}'")));
    }

    // The gate is journaled; signals arrive in completion order, not
    // await order (children finish when they finish), so the scan is
    // forward from the gate.
    let due = attempt.pending.front().expect("checked").data["due_ms"].as_i64();
    let matched = attempt.pending.iter().skip(1).position(wanted);
    if let Some(offset) = matched {
        attempt.pending.pop_front();
        let signal = attempt
            .pending
            .remove(offset)
            .expect("position came from this deque");
        let winner = signal.data["name"].as_str().map(str::to_owned);
        return Ok((lua.to_value(&signal.data["payload"])?, winner));
    }
    match due {
        Some(due) if due <= now => {
            attempt.pending.pop_front();
            Ok((mlua::Value::Nil, None))
        }
        Some(due) => {
            arm(attempt, due - now).map_err(mlua::Error::RuntimeError)?;
            drop(guard);
            Err(shared.park(format!("awaiting '{describe}'")))
        }
        None => {
            drop(guard);
            Err(shared.park(format!("awaiting '{describe}'")))
        }
    }
}

/// The `wf` handle the run body receives; every verb consults the
/// shared cursor, which is what makes replay transparent.
struct WfHandle;

impl mlua::UserData for WfHandle {
    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        use mlua::LuaSerdeExt;
        // step(name, fn) or step(name, opts, fn); opts (retries, backoff,
        // timeout) are accepted and recorded but not yet enforced.
        methods.add_async_method(
            "step",
            |lua, _this, (name, a, b): (String, mlua::Value, Option<mlua::Function>)| async move {
                let (body, options) = match (&a, b) {
                    (mlua::Value::Function(function), _) => (function.clone(), None),
                    (mlua::Value::Table(options), Some(function)) => {
                        (function, Some(options.clone()))
                    }
                    _ => {
                        return Err(mlua::Error::RuntimeError(
                            "wf:step takes a name, optional options and a function.".to_owned(),
                        ));
                    }
                };
                // Enforced options: retries is the total attempt count,
                // backoff doubles per attempt, timeout bounds the body.
                let retries: i64 = options
                    .as_ref()
                    .and_then(|table| table.get("retries").ok())
                    .unwrap_or(1i64)
                    .max(1);
                let backoff_ms = options
                    .as_ref()
                    .and_then(|table| table.get::<mlua::Value>("backoff").ok())
                    .map(|value| match value {
                        mlua::Value::String(raw) => {
                            crate::extensions::objects::parse_duration_ms(&raw.to_str()?)
                                .map_err(mlua::Error::RuntimeError)
                        }
                        mlua::Value::Integer(seconds) => Ok(seconds * 1000),
                        mlua::Value::Nil => Ok(2000),
                        _ => Err(mlua::Error::RuntimeError(
                            "backoff is a duration.".to_owned(),
                        )),
                    })
                    .transpose()?
                    .unwrap_or(2000);
                let timeout_ms = options
                    .as_ref()
                    .and_then(|table| table.get::<mlua::Value>("timeout").ok())
                    .map(|value| match value {
                        mlua::Value::String(raw) => {
                            crate::extensions::objects::parse_duration_ms(&raw.to_str()?)
                                .map_err(mlua::Error::RuntimeError)
                        }
                        mlua::Value::Integer(seconds) => Ok(seconds * 1000),
                        mlua::Value::Nil => Ok(0),
                        _ => Err(mlua::Error::RuntimeError(
                            "timeout is a duration.".to_owned(),
                        )),
                    })
                    .transpose()?
                    .unwrap_or(0);

                let shared = lua
                    .app_data_ref::<std::sync::Arc<WfShared>>()
                    .map(|shared| shared.clone())
                    .ok_or_else(|| mlua::Error::RuntimeError("Not a workflow vm.".to_owned()))?;
                if shared.effects_allowed() {
                    return Err(mlua::Error::RuntimeError(
                        "Steps do not nest; perform one effect per step.".to_owned(),
                    ));
                }

                // Walk the cursor through this step's history: INTENT and
                // non-final FAILED rows count attempts; RESULT replays; a
                // final FAILED blocks unless this attempt is a resume.
                enum Plan {
                    Replay(serde_json::Value),
                    Run { attempt: i64 },
                    Blocked(String),
                }
                let plan = {
                    let mut guard = shared.attempt.lock().expect("no poisoned lock");
                    let attempt = guard.as_mut().ok_or_else(|| {
                        mlua::Error::RuntimeError("No workflow attempt is executing.".to_owned())
                    })?;

                    let mut attempts_seen: i64 = 0;
                    let mut plan = None;
                    while plan.is_none() {
                        set_aside_leading_signals(&mut attempt.pending);
                        match attempt.pending.front() {
                            Some(entry)
                                if entry.kind == EntryKind::Intent
                                    && entry.data["step"] == name.as_str() =>
                            {
                                attempts_seen += 1;
                                attempt.pending.pop_front();
                            }
                            Some(entry)
                                if entry.kind == EntryKind::Failed
                                    && entry.data["step"] == name.as_str() =>
                            {
                                let is_final = entry.data["final"].as_bool().unwrap_or(false);
                                let error =
                                    entry.data["error"].as_str().unwrap_or("failed").to_owned();
                                let trailing = attempt.pending.len() == 1;
                                if is_final && trailing && !attempt.resume {
                                    plan = Some(Plan::Blocked(error));
                                } else if is_final && trailing && attempt.resume {
                                    // The resume consumes the verdict and
                                    // starts a fresh attempt sequence.
                                    attempt.pending.pop_front();
                                    attempt.resume = false;
                                    attempts_seen = 0;
                                    plan = Some(Plan::Run { attempt: 1 });
                                } else {
                                    // A historical failure (retried past,
                                    // or resumed long ago): consumed.
                                    attempt.pending.pop_front();
                                }
                            }
                            Some(entry)
                                if entry.kind == EntryKind::Result
                                    && entry.data["step"] == name.as_str() =>
                            {
                                let value = entry.data["value"].clone();
                                attempt.pending.pop_front();
                                plan = Some(Plan::Replay(value));
                            }
                            Some(entry)
                                if attempts_seen == 0 && entry.kind != EntryKind::Signal =>
                            {
                                return Err(mlua::Error::RuntimeError(format!(
                                    "journal divergence: expected {:?}, code reached step '{name}'",
                                    entry.kind
                                )));
                            }
                            // Attempts consumed and nothing decided the
                            // step: this run attempt continues it.
                            _ => {
                                plan = Some(Plan::Run {
                                    attempt: attempts_seen.max(0) + 1,
                                })
                            }
                        }
                    }
                    let plan = plan.expect("loop decides");

                    if let Plan::Run { attempt: number } = plan {
                        // A fresh attempt journals its intent before the
                        // effect: persist-intent, do, persist-result.
                        // Replayed attempts already journaled theirs.
                        if number > attempts_seen {
                            attempt
                                .home
                                .with_storage(|storage| {
                                    append(
                                        storage,
                                        EntryKind::Intent,
                                        &serde_json::json!({ "step": name, "attempt": number }),
                                    )?;
                                    storage.commit()?;
                                    storage.begin()
                                })
                                .map_err(mlua::Error::RuntimeError)?;
                        }
                    }
                    plan
                };

                match plan {
                    Plan::Blocked(error) => Err(shared.fail(format!(
                        "step '{name}' failed after {retries} attempts: {error}"
                    ))),
                    Plan::Replay(value) => lua.to_value(&value),
                    Plan::Run { attempt: number } => {
                        // A test fake stands in for the body but walks
                        // the same journal path, so replay is identical.
                        let fake = shared
                            .fakes
                            .lock()
                            .expect("no poisoned lock")
                            .get(&name)
                            .cloned();
                        shared
                            .in_step
                            .store(true, std::sync::atomic::Ordering::Relaxed);
                        let outcome: Result<mlua::Value, mlua::Error> = if let Some(value) = fake {
                            lua.to_value(&value)
                        } else if timeout_ms > 0 {
                            match tokio::time::timeout(
                                std::time::Duration::from_millis(timeout_ms as u64),
                                body.call_async(()),
                            )
                            .await
                            {
                                Ok(value) => value,
                                Err(_) => Err(mlua::Error::RuntimeError(format!(
                                    "step '{name}' timed out after {timeout_ms}ms"
                                ))),
                            }
                        } else {
                            body.call_async(()).await
                        };
                        shared
                            .in_step
                            .store(false, std::sync::atomic::Ordering::Relaxed);

                        let shared_after = lua
                            .app_data_ref::<std::sync::Arc<WfShared>>()
                            .map(|handle| handle.clone())
                            .expect("checked above");
                        match outcome {
                            Ok(value) => {
                                let json: serde_json::Value = lua.from_value(value.clone())?;
                                let guard = shared_after.attempt.lock().expect("no poisoned lock");
                                let attempt = guard.as_ref().expect("attempt is executing");
                                attempt
                                    .home
                                    .with_storage(|storage| {
                                        append(
                                            storage,
                                            EntryKind::Result,
                                            &serde_json::json!({
                                                "step": name,
                                                "value": json,
                                                "attempt": number,
                                            }),
                                        )?;
                                        storage.commit()?;
                                        storage.begin()
                                    })
                                    .map_err(mlua::Error::RuntimeError)?;
                                Ok(value)
                            }
                            Err(error) => {
                                let text = error.to_string();
                                let exhausted = number >= retries;
                                {
                                    let guard =
                                        shared_after.attempt.lock().expect("no poisoned lock");
                                    let attempt = guard.as_ref().expect("attempt is executing");
                                    attempt
                                        .home
                                        .with_storage(|storage| {
                                            append(
                                                storage,
                                                EntryKind::Failed,
                                                &serde_json::json!({
                                                    "step": name,
                                                    "attempt": number,
                                                    "error": text,
                                                    "final": exhausted,
                                                }),
                                            )?;
                                            storage.commit()?;
                                            storage.begin()
                                        })
                                        .map_err(mlua::Error::RuntimeError)?;
                                }
                                if exhausted {
                                    return Err(shared_after.fail(format!(
                                        "step '{name}' failed after {retries} attempts: {text}"
                                    )));
                                }
                                // Durable backoff: the alarm wakes the
                                // replay, which re-reaches this step and
                                // runs the next attempt.
                                let wait = backoff_ms.saturating_mul(1 << (number - 1).min(16));
                                {
                                    let guard =
                                        shared_after.attempt.lock().expect("no poisoned lock");
                                    let attempt = guard.as_ref().expect("attempt is executing");
                                    arm(attempt, wait).map_err(mlua::Error::RuntimeError)?;
                                }
                                Err(shared_after.park(format!(
                                    "step '{name}' attempt {number} failed; retrying in {wait}ms"
                                )))
                            }
                        }
                    }
                }
            },
        );

        methods.add_method("sleep", |lua, _this, duration: mlua::Value| {
            let delay_ms = match &duration {
                mlua::Value::String(raw) => {
                    crate::extensions::objects::parse_duration_ms(&raw.to_str()?)
                        .map_err(mlua::Error::RuntimeError)?
                }
                mlua::Value::Integer(seconds) => seconds * 1000,
                mlua::Value::Number(seconds) => (*seconds * 1000.0) as i64,
                _ => {
                    return Err(mlua::Error::RuntimeError(
                        "wf:sleep takes a duration: \"30s\", \"10m\" or seconds.".to_owned(),
                    ));
                }
            };
            let shared = lua
                .app_data_ref::<std::sync::Arc<WfShared>>()
                .map(|shared| shared.clone())
                .ok_or_else(|| mlua::Error::RuntimeError("Not a workflow vm.".to_owned()))?;

            let mut guard = shared.attempt.lock().expect("no poisoned lock");
            let attempt = guard.as_mut().ok_or_else(|| {
                mlua::Error::RuntimeError("No workflow attempt is executing.".to_owned())
            })?;
            let now = crate::extensions::objects::unix_now_ms();

            set_aside_leading_signals(&mut attempt.pending);
            match attempt
                .pending
                .front()
                .filter(|entry| entry.kind != EntryKind::Signal)
            {
                Some(entry) if entry.kind == EntryKind::Timer && entry.data["for"].is_null() => {
                    let due = entry.data["due_ms"].as_i64().unwrap_or(0);
                    if due <= now {
                        attempt.pending.pop_front();
                        return Ok(());
                    }
                    arm(attempt, due - now).map_err(mlua::Error::RuntimeError)?;
                    drop(guard);
                    Err(shared.park(format!("sleeping, due in {}ms", due - now)))
                }
                Some(entry) => Err(mlua::Error::RuntimeError(format!(
                    "journal divergence: expected {:?}, code reached sleep",
                    entry.kind
                ))),
                None => {
                    let due = now + delay_ms.max(0);
                    attempt
                        .home
                        .with_storage(|storage| {
                            append(
                                storage,
                                EntryKind::Timer,
                                &serde_json::json!({ "due_ms": due, "for": null }),
                            )?;
                            storage.commit()?;
                            storage.begin()
                        })
                        .map_err(mlua::Error::RuntimeError)?;
                    arm(attempt, delay_ms.max(0)).map_err(mlua::Error::RuntimeError)?;
                    drop(guard);
                    Err(shared.park(format!("sleeping {delay_ms}ms")))
                }
            }
        });

        // await(name, opts?): parks until the named signal arrives or
        // the timeout passes; nil on timeout.
        methods.add_method(
            "await",
            |lua, _this, (name, opts): (String, Option<mlua::Table>)| {
                let timeout_ms = parse_timeout(opts)?;
                let (payload, _winner) =
                    await_signals(lua, std::slice::from_ref(&name), timeout_ms)?;
                Ok(payload)
            },
        );

        // all { a, b, ... }: joins every entry (a spawned job or a bare
        // signal name), returning payloads in ARGUMENT order however the
        // completions arrive. A timeout applies per join; an entry that
        // times out reads as nil in the results.
        methods.add_method(
            "all",
            |lua, _this, (jobs, opts): (mlua::Table, Option<mlua::Table>)| {
                let timeout_ms = parse_timeout(opts)?;
                let names = signal_names(&jobs)?;
                let results = lua.create_table()?;
                for (index, name) in names.iter().enumerate() {
                    let (payload, _winner) =
                        await_signals(lua, std::slice::from_ref(name), timeout_ms)?;
                    results.set(index + 1, payload)?;
                }
                Ok(results)
            },
        );

        // race { a, b, ... }: the first completion or signal among the
        // set wins; returns (payload, winner name), or (nil, nil) on
        // timeout. Losers keep running; their signals stay consumable.
        methods.add_method(
            "race",
            |lua, _this, (jobs, opts): (mlua::Table, Option<mlua::Table>)| {
                let timeout_ms = parse_timeout(opts)?;
                let names = signal_names(&jobs)?;
                if names.is_empty() {
                    return Err(mlua::Error::RuntimeError(
                        "race takes at least one job or signal name.".to_owned(),
                    ));
                }
                let (payload, winner) = await_signals(lua, &names, timeout_ms)?;
                Ok((payload, winner))
            },
        );

        // spawn(definition, input): a child run whose id derives from
        // this run's; the CHILD row is the deterministic record, the
        // start dispatch is the effect. Returns a job handle whose
        // completion signal `wf:all` awaits.
        methods.add_async_method(
            "spawn",
            |lua, _this, (definition, input): (String, mlua::Value)| async move {
                let shared = lua
                    .app_data_ref::<std::sync::Arc<WfShared>>()
                    .map(|shared| shared.clone())
                    .ok_or_else(|| {
                        mlua::Error::RuntimeError("Not a workflow vm.".to_owned())
                    })?;
                let input_json: serde_json::Value = lua.from_value(input)?;

                enum Plan {
                    Replay(String),
                    Launch(String, String),
                }
                let plan = {
                    let mut guard = shared.attempt.lock().expect("no poisoned lock");
                    let attempt = guard.as_mut().ok_or_else(|| {
                        mlua::Error::RuntimeError("No workflow attempt is executing.".to_owned())
                    })?;
                    set_aside_leading_signals(&mut attempt.pending);
                    match attempt
                        .pending
                        .front()
                        .filter(|entry| entry.kind != EntryKind::Signal)
                    {
                        Some(entry)
                            if entry.kind == EntryKind::Child
                                && entry.data["definition"] == definition.as_str() =>
                        {
                            let child = entry.data["child"]
                                .as_str()
                                .unwrap_or_default()
                                .to_owned();
                            attempt.pending.pop_front();
                            Plan::Replay(child)
                        }
                        Some(entry) => {
                            return Err(mlua::Error::RuntimeError(format!(
                                "journal divergence: expected {:?}, code reached spawn '{definition}'",
                                entry.kind
                            )));
                        }
                        None => {
                            // The ordinal makes the child id deterministic
                            // AND unique per spawn site.
                            let ordinal = attempt
                                .home
                                .with_storage(|storage| {
                                    let entries = read_from(storage, 0)?;
                                    Ok(entries
                                        .iter()
                                        .filter(|e| e.kind == EntryKind::Child)
                                        .count())
                                })
                                .map_err(mlua::Error::RuntimeError)?;
                            let run_id = attempt
                                .name
                                .split('/')
                                .nth(1)
                                .unwrap_or("run")
                                .to_owned();
                            let child = format!("{definition}/{run_id}.{ordinal}");
                            attempt
                                .home
                                .with_storage(|storage| {
                                    append(
                                        storage,
                                        EntryKind::Child,
                                        &serde_json::json!({
                                            "definition": definition,
                                            "child": child,
                                            "input": input_json,
                                        }),
                                    )?;
                                    storage.commit()?;
                                    storage.begin()
                                })
                                .map_err(mlua::Error::RuntimeError)?;
                            Plan::Launch(child, attempt.name.clone())
                        }
                    }
                };

                let child_name = match plan {
                    Plan::Replay(child) => child,
                    Plan::Launch(child, parent) => {
                        let router = lua
                            .app_data_ref::<crate::extensions::objects::ObjectRouter>()
                            .map(|router| router.clone())
                            .ok_or_else(|| {
                                mlua::Error::RuntimeError(
                                    "Objects are not available in this runtime.".to_owned(),
                                )
                            })?;
                        let (definition_part, name_part) =
                            child.split_once('/').unwrap_or((child.as_str(), ""));
                        router(crate::extensions::objects::ObjectTarget {
                            class: actias_common::classes::WORKFLOW_CLASS.to_owned(),
                            name: format!("{definition_part}/{name_part}"),
                            method: "start".to_owned(),
                            arguments: vec![input_json.clone(), serde_json::json!(parent)],
                            chain: Vec::new(),
                            caller: None,
                        })
                        .await
                        .map_err(mlua::Error::RuntimeError)?;
                        child
                    }
                };

                let job = lua.create_table()?;
                job.set("name", child_name.clone())?;
                job.set("signal", format!("__child:{child_name}"))?;
                Ok(job)
            },
        );
    }
}

/// Tells a parent run its child reached a terminal state, as the
/// `__child:<name>` signal `wf:all` awaits. Best effort: a missing
/// router (embedded runs) or a gone parent only logs.
async fn notify_parent(
    runtime: &crate::runtime::ActiasRuntime,
    parent: &Option<String>,
    own_name: &str,
    payload: serde_json::Value,
) {
    let Some(parent) = parent.clone() else { return };
    let Some(router) = runtime
        .app_data_ref::<crate::extensions::objects::ObjectRouter>()
        .map(|router| router.clone())
    else {
        return;
    };
    let signal_name = format!("__child:{own_name}");
    // Fire and forget: a child completing INSIDE its parent's spawn call
    // would deadlock the parent's mailbox if this awaited inline. The
    // signal row is the durable handoff; delivery drives the wake.
    tokio::spawn(async move {
        let outcome = router(crate::extensions::objects::ObjectTarget {
            class: actias_common::classes::WORKFLOW_CLASS.to_owned(),
            name: parent.clone(),
            method: "signal".to_owned(),
            arguments: vec![serde_json::json!(signal_name), payload],
            chain: Vec::new(),
            caller: None,
        })
        .await;
        if let Err(error) = outcome {
            actias_common::tracing::warn!(
                %error, parent, "child completion did not reach its parent"
            );
        }
    });
}

/// Arms the instance's one alarm through both homes, like any platform
/// class arming from inside a call.
fn arm(attempt: &Attempt, delay_ms: i64) -> Result<(), String> {
    attempt
        .home
        .set_alarm(crate::extensions::objects::PendingAlarm {
            due_ms: crate::extensions::objects::unix_now_ms() + delay_ms.max(0),
            class: actias_common::classes::WORKFLOW_CLASS.to_owned(),
            name: attempt.name.clone(),
            own_key: attempt.own_key.clone(),
        })
}

/// Runs one platform method against a workflow instance.
///
/// # Errors
/// Returns user-safe texts like every platform class.
pub(crate) async fn dispatch(
    runtime: &crate::runtime::ActiasRuntime,
    context: &super::PlatformContext<'_>,
    call: &super::Call,
) -> Result<serde_json::Value, String> {
    context.home.with_storage(ensure_schema)?;

    match call.method.as_str() {
        "start" => {
            run_attempt(
                runtime,
                context,
                Some((
                    call.args
                        .first()
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                    call.args
                        .get(1)
                        .and_then(|value| value.as_str())
                        .map(str::to_owned),
                )),
                false,
            )
            .await
        }
        // A resume re-enters at the failed step with fresh attempts;
        // everything before it replays untouched.
        "resume" => run_attempt(runtime, context, None, true).await,
        // The alarm is a wake: replay to the parked verb, which now
        // finds its timer due (or its signal arrived) and continues.
        "alarm" => run_attempt(runtime, context, None, false).await,
        "signal" => {
            let name = call
                .args
                .first()
                .and_then(|value| value.as_str())
                .ok_or_else(|| "signal takes a name and an optional payload.".to_owned())?
                .to_owned();
            let payload = call.args.get(1).cloned().unwrap_or(serde_json::Value::Null);
            context.home.with_storage(|storage| {
                append(
                    storage,
                    EntryKind::Signal,
                    &serde_json::json!({ "name": name, "payload": payload }),
                )
            })?;
            run_attempt(runtime, context, None, false).await
        }
        "cancel" => {
            let reason = call
                .args
                .first()
                .and_then(|value| value.as_str())
                .unwrap_or("cancelled")
                .to_owned();
            let children: Vec<String> = context.home.with_storage(|storage| {
                let entries = read_from(storage, 0)?;
                append(
                    storage,
                    EntryKind::Cancel,
                    &serde_json::json!({ "reason": reason }),
                )?;
                Ok(entries
                    .iter()
                    .filter(|e| e.kind == EntryKind::Child)
                    .filter_map(|e| e.data["child"].as_str().map(str::to_owned))
                    .collect())
            })?;
            // Cancellation is structured: every spawned child gets the
            // same verdict, best effort, before the caller hears ours.
            if let Some(router) = runtime
                .app_data_ref::<crate::extensions::objects::ObjectRouter>()
                .map(|router| router.clone())
            {
                for child in children {
                    let outcome = router(crate::extensions::objects::ObjectTarget {
                        class: actias_common::classes::WORKFLOW_CLASS.to_owned(),
                        name: child.clone(),
                        method: "cancel".to_owned(),
                        arguments: vec![serde_json::json!(reason.clone())],
                        chain: Vec::new(),
                        caller: None,
                    })
                    .await;
                    if let Err(error) = outcome {
                        actias_common::tracing::warn!(%error, child, "cancel did not reach the child");
                    }
                }
            }
            Ok(serde_json::json!({ "status": "cancelled", "reason": reason }))
        }
        "status" => {
            let head = context.home.with_storage(head)?;
            Ok(head
                .map(|entry| serde_json::json!({ "kind": entry.kind, "seq": entry.seq, "at": entry.at }))
                .unwrap_or(serde_json::Value::Null))
        }
        other => Err(format!(
            "Object class '{}' has no method '{other}'.",
            actias_common::classes::WORKFLOW_CLASS
        )),
    }
}

/// One run attempt: replay the journal from the top, continue live past
/// its end, journal the return. Joining a completed run returns the
/// recorded outcome, which is what makes `start` idempotent; a parked
/// verb unwinds here and reads as parked, never as failure. `input` is
/// [`Some`] only for `start`, which may create the run.
async fn run_attempt(
    runtime: &crate::runtime::ActiasRuntime,
    context: &super::PlatformContext<'_>,
    input: Option<(serde_json::Value, Option<String>)>,
    resume: bool,
) -> Result<serde_json::Value, String> {
    let entries = context.home.with_storage(|storage| read_from(storage, 0))?;

    if let Some(done) = entries
        .iter()
        .find(|entry| entry.kind == EntryKind::Completed)
    {
        return Ok(serde_json::json!({ "status": "completed", "value": done.data["value"] }));
    }
    if let Some(cancelled) = entries.iter().find(|entry| entry.kind == EntryKind::Cancel) {
        return Ok(
            serde_json::json!({ "status": "cancelled", "reason": cancelled.data["reason"] }),
        );
    }
    // A wake or signal on a run that never started is a stale alarm or a
    // caller racing creation; both read as nothing to do.
    if entries.is_empty() && input.is_none() {
        return Ok(serde_json::Value::Null);
    }
    let (input, parent_arg) = input.unwrap_or((serde_json::Value::Null, None));

    // The definition is the instance name's first segment; the caller id
    // after it is the run's identity.
    let definition = context
        .name
        .split('/')
        .next()
        .unwrap_or_default()
        .to_owned();
    let listener = runtime
        .listener(&format!(
            "{}{definition}",
            crate::runtime::ActiasRuntime::WORKFLOW_EVENT_PREFIX
        ))
        .map_err(|_| format!("No workflow '{definition}' is declared by the owning script."))?;

    let (seed, input, parent, pending) = match entries.first() {
        Some(started) if started.kind == EntryKind::Started => (
            started.data["seed"].as_i64().unwrap_or(1) as u64,
            started.data["input"].clone(),
            started.data["parent"].as_str().map(str::to_owned),
            entries[1..].to_vec(),
        ),
        Some(other) => {
            return Err(format!(
                "journal divergence: first entry is {:?}, not STARTED",
                other.kind
            ));
        }
        None => {
            let seed = uuid::Uuid::new_v4().as_u128() as u64 | 1;
            let revision = context
                .home
                .revision()
                .map(|prepared| prepared.revision_id.clone())
                .unwrap_or_default();
            context.home.with_storage(|storage| {
                append(
                    storage,
                    EntryKind::Started,
                    &serde_json::json!({
                        "input": input,
                        "seed": seed,
                        "revision": revision,
                        "engine": "luau",
                        "parent": parent_arg,
                    }),
                )
            })?;
            (seed, input, parent_arg, Vec::new())
        }
    };

    let shared = runtime
        .app_data_ref::<std::sync::Arc<WfShared>>()
        .map(|shared| shared.clone())
        .ok_or_else(|| "This vm has no workflow cursor; not a workflow vm.".to_owned())?;
    *shared.attempt.lock().expect("no poisoned lock") = Some(Attempt {
        pending: pending.into(),
        home: runtime
            .app_data_ref::<std::sync::Arc<crate::objects::ObjectHome>>()
            .map(|home| home.clone())
            .ok_or_else(|| "This vm has no object home.".to_owned())?,
        rng: seed,
        own_key: context.own_key.to_owned(),
        name: context.name.to_owned(),
        resume,
    });

    let outcome: Result<mlua::Value, mlua::Error> = {
        use mlua::LuaSerdeExt;
        let argument = runtime
            .to_value(&input)
            .map_err(|e| format!("workflow input did not convert: {e}"))?;
        listener.call_async((WfHandle, argument)).await
    };
    *shared.attempt.lock().expect("no poisoned lock") = None;

    match outcome {
        Ok(value) => {
            use mlua::LuaSerdeExt;
            let json: serde_json::Value = runtime
                .from_value(value)
                .map_err(|e| format!("workflow return did not convert: {e}"))?;
            context.home.with_storage(|storage| {
                append(
                    storage,
                    EntryKind::Completed,
                    &serde_json::json!({ "value": json }),
                )
            })?;
            notify_parent(
                runtime,
                &parent,
                context.name,
                serde_json::json!({ "status": "completed", "value": json }),
            )
            .await;
            Ok(serde_json::json!({ "status": "completed", "value": json }))
        }
        Err(error) => {
            // A park is progress, not failure: the verb recorded why
            // before unwinding, and the journaled gate plus the armed
            // alarm are already durable.
            if let Some(reason) = shared.parked.lock().expect("no poisoned lock").take() {
                return Ok(serde_json::json!({ "status": "parked", "reason": reason }));
            }
            // A failed run is a state, not a transport error: the
            // journal holds the verdict and a resume re-enters it.
            let failed_reason = shared.failed.lock().expect("no poisoned lock").take();
            if let Some(reason) = failed_reason {
                notify_parent(
                    runtime,
                    &parent,
                    context.name,
                    serde_json::json!({ "status": "failed", "reason": reason }),
                )
                .await;
                return Ok(serde_json::json!({ "status": "failed", "reason": reason }));
            }
            Err(error.to_string())
        }
    }
}
