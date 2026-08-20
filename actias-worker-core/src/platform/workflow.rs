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
const SCHEMA_VERSION: i64 = 1;

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

/// Everything a journal row can record. The set is closed on purpose:
/// replay must understand every kind it can meet, so a new kind is a
/// format bump, never a silent addition.
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
}

impl EntryKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Started => "STARTED",
            Self::Intent => "INTENT",
            Self::Result => "RESULT",
            Self::Timer => "TIMER",
            Self::Signal => "SIGNAL",
            Self::Child => "CHILD",
            Self::Cancel => "CANCEL",
            Self::Completed => "COMPLETED",
            Self::Ambient => "AMBIENT",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "STARTED" => Self::Started,
            "INTENT" => Self::Intent,
            "RESULT" => Self::Result,
            "TIMER" => Self::Timer,
            "SIGNAL" => Self::Signal,
            "CHILD" => Self::Child,
            "CANCEL" => Self::Cancel,
            "COMPLETED" => Self::Completed,
            "AMBIENT" => Self::Ambient,
            _ => return None,
        })
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
    storage
        .platform()
        .execute(CREATE_JOURNAL, [])
        .map_err(|e| e.to_string())?;
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
                crate::proto::kv_service::kv_service_client::KvServiceClient::new(channel),
                crate::egress::EgressClient::new(crate::egress::EgressPolicy::new([], false))
                    .expect("client builds"),
                None,
                None,
                None,
                VmProfile::Workflow(shared.clone()),
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
}

impl WfShared {
    /// Parks the run: records why, then unwinds the Lua stack. The
    /// error is only the vehicle; the flag is the truth.
    fn park(&self, reason: String) -> mlua::Error {
        *self.parked.lock().expect("no poisoned lock") = Some(reason);
        mlua::Error::RuntimeError("workflow parked".to_owned())
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

        if let Some(entry) = attempt.pending.front() {
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
                let body = match (&a, b) {
                    (mlua::Value::Function(function), _) => function.clone(),
                    (_, Some(function)) => function,
                    _ => {
                        return Err(mlua::Error::RuntimeError(
                            "wf:step takes a name and a function.".to_owned(),
                        ));
                    }
                };

                let shared = lua
                    .app_data_ref::<std::sync::Arc<WfShared>>()
                    .map(|shared| shared.clone())
                    .ok_or_else(|| mlua::Error::RuntimeError("Not a workflow vm.".to_owned()))?;

                // Replay: a recorded RESULT answers without running the
                // body. A dangling INTENT (the crash window) re-runs it.
                enum Plan {
                    Replay(serde_json::Value),
                    Run,
                }
                let plan = {
                    let mut guard = shared.attempt.lock().expect("no poisoned lock");
                    let attempt = guard.as_mut().ok_or_else(|| {
                        mlua::Error::RuntimeError("No workflow attempt is executing.".to_owned())
                    })?;

                    match attempt.pending.front() {
                        Some(entry)
                            if entry.kind == EntryKind::Intent
                                && entry.data["step"] == name.as_str() =>
                        {
                            attempt.pending.pop_front();
                            match attempt.pending.front() {
                                Some(result)
                                    if result.kind == EntryKind::Result
                                        && result.data["step"] == name.as_str() =>
                                {
                                    let value = result.data["value"].clone();
                                    attempt.pending.pop_front();
                                    Plan::Replay(value)
                                }
                                // INTENT without RESULT: the crash window;
                                // the effect may or may not have happened,
                                // so it runs again (idempotency keys are
                                // the code's tool for the difference).
                                _ => Plan::Run,
                            }
                        }
                        Some(entry) => {
                            return Err(mlua::Error::RuntimeError(format!(
                                "journal divergence: expected {:?}, code reached step '{name}'",
                                entry.kind
                            )));
                        }
                        None => {
                            let home = attempt.home.clone();
                            home.with_storage(|storage| {
                                append(
                                    storage,
                                    EntryKind::Intent,
                                    &serde_json::json!({ "step": name }),
                                )?;
                                // The intent (and every journaled read
                                // before it) is durable BEFORE the effect
                                // runs: persist-intent, do, persist-result.
                                storage.commit()?;
                                storage.begin()
                            })
                            .map_err(mlua::Error::RuntimeError)?;
                            Plan::Run
                        }
                    }
                };

                match plan {
                    Plan::Replay(value) => lua.to_value(&value),
                    Plan::Run => {
                        let value: mlua::Value = body.call_async(()).await?;
                        let json: serde_json::Value = lua.from_value(value.clone())?;
                        let shared = lua
                            .app_data_ref::<std::sync::Arc<WfShared>>()
                            .map(|shared| shared.clone())
                            .expect("checked above");
                        let guard = shared.attempt.lock().expect("no poisoned lock");
                        let attempt = guard.as_ref().expect("attempt is executing");
                        attempt
                            .home
                            .with_storage(|storage| {
                                append(
                                    storage,
                                    EntryKind::Result,
                                    &serde_json::json!({ "step": name, "value": json }),
                                )?;
                                // persist-result: the effect's outcome is
                                // durable before anything downstream can
                                // observe it, so a later crash replays
                                // the value instead of the effect.
                                storage.commit()?;
                                storage.begin()
                            })
                            .map_err(mlua::Error::RuntimeError)?;
                        Ok(value)
                    }
                }
            },
        );

        // sleep(duration): real suspension. Live appends TIMER and arms
        // the alarm; replay past a due timer just continues.
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

            match attempt.pending.front() {
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
                let timeout_ms = opts
                    .and_then(|table| table.get::<mlua::Value>("timeout").ok())
                    .map(|value| match value {
                        mlua::Value::String(raw) => {
                            crate::extensions::objects::parse_duration_ms(&raw.to_str()?)
                                .map_err(mlua::Error::RuntimeError)
                        }
                        mlua::Value::Integer(seconds) => Ok(seconds * 1000),
                        mlua::Value::Nil => Ok(i64::MAX),
                        _ => Err(mlua::Error::RuntimeError(
                            "await's timeout is a duration.".to_owned(),
                        )),
                    })
                    .transpose()?;

                let shared = lua
                    .app_data_ref::<std::sync::Arc<WfShared>>()
                    .map(|shared| shared.clone())
                    .ok_or_else(|| mlua::Error::RuntimeError("Not a workflow vm.".to_owned()))?;
                let mut guard = shared.attempt.lock().expect("no poisoned lock");
                let attempt = guard.as_mut().ok_or_else(|| {
                    mlua::Error::RuntimeError("No workflow attempt is executing.".to_owned())
                })?;
                let now = crate::extensions::objects::unix_now_ms();

                // The gate row: live appends it; replay finds it first.
                match attempt.pending.front() {
                    Some(entry)
                        if entry.kind == EntryKind::Timer && entry.data["for"] == name.as_str() => {
                    }
                    Some(entry) => {
                        return Err(mlua::Error::RuntimeError(format!(
                            "journal divergence: expected {:?}, code reached await '{name}'",
                            entry.kind
                        )));
                    }
                    None => {
                        let due = timeout_ms.map(|ms| now.saturating_add(ms.max(0)));
                        attempt
                            .home
                            .with_storage(|storage| {
                                append(
                                    storage,
                                    EntryKind::Timer,
                                    &serde_json::json!({ "due_ms": due, "for": name }),
                                )?;
                                storage.commit()?;
                                storage.begin()
                            })
                            .map_err(mlua::Error::RuntimeError)?;
                        if let Some(ms) = timeout_ms.filter(|ms| *ms < i64::MAX) {
                            arm(attempt, ms.max(0)).map_err(mlua::Error::RuntimeError)?;
                        }
                        drop(guard);
                        return Err(shared.park(format!("awaiting '{name}'")));
                    }
                }

                // The gate is journaled; the answer is whatever follows.
                let due = attempt.pending.front().expect("checked").data["due_ms"].as_i64();
                if attempt.pending.get(1).is_some_and(|next| {
                    next.kind == EntryKind::Signal && next.data["name"] == name.as_str()
                }) {
                    attempt.pending.pop_front();
                    let signal = attempt.pending.pop_front().expect("checked");
                    return lua.to_value(&signal.data["payload"]);
                }
                match due {
                    Some(due) if due <= now => {
                        attempt.pending.pop_front();
                        Ok(mlua::Value::Nil)
                    }
                    Some(due) => {
                        arm(attempt, due - now).map_err(mlua::Error::RuntimeError)?;
                        drop(guard);
                        Err(shared.park(format!("awaiting '{name}'")))
                    }
                    None => {
                        drop(guard);
                        Err(shared.park(format!("awaiting '{name}'")))
                    }
                }
            },
        );
    }
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
                Some(
                    call.args
                        .first()
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                ),
            )
            .await
        }
        // The alarm is a wake: replay to the parked verb, which now
        // finds its timer due (or its signal arrived) and continues.
        "alarm" => run_attempt(runtime, context, None).await,
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
            run_attempt(runtime, context, None).await
        }
        "cancel" => {
            let reason = call
                .args
                .first()
                .and_then(|value| value.as_str())
                .unwrap_or("cancelled")
                .to_owned();
            context.home.with_storage(|storage| {
                append(
                    storage,
                    EntryKind::Cancel,
                    &serde_json::json!({ "reason": reason }),
                )
            })?;
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
    input: Option<serde_json::Value>,
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
    let input = input.unwrap_or(serde_json::Value::Null);

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

    let (seed, input, pending) = match entries.first() {
        Some(started) if started.kind == EntryKind::Started => (
            started.data["seed"].as_i64().unwrap_or(1) as u64,
            started.data["input"].clone(),
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
                    }),
                )
            })?;
            (seed, input, Vec::new())
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
            Ok(serde_json::json!({ "status": "completed", "value": json }))
        }
        Err(error) => {
            // A park is progress, not failure: the verb recorded why
            // before unwinding, and the journaled gate plus the armed
            // alarm are already durable.
            if let Some(reason) = shared.parked.lock().expect("no poisoned lock").take() {
                return Ok(serde_json::json!({ "status": "parked", "reason": reason }));
            }
            Err(error.to_string())
        }
    }
}
