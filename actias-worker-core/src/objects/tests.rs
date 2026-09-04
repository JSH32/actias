//! The object tests.

use super::*;
use crate::proto::bundle::{Bundle, File};
use crate::proto::kv_service::kv_service_client::KvServiceClient;
use crate::proto::script_service::{Revision, Script};
use crate::runtime::PreparedRevision;
use std::sync::Arc;

/// A pinned runtime whose entry point is `source`; clients are
/// unconnectable, so tests exercise only the vm.
use super::testing::{runtime_with, runtime_with_files};

/// A pinned runtime with storage, for the directory tests below.
async fn stored_object(source: &str, dir: &std::path::Path) -> ObjectHandle {
    let runtime = runtime_with(source).await;
    spawn_object_task(
        runtime,
        TaskOptions {
            storage: Some(
                crate::storage::SqliteStorage::open(&dir.join("object.db")).expect("opens"),
            ),
            ..Default::default()
        },
    )
}

/// The row a call derived, read back through the kernel's own
/// accessor the way the shipper will.
fn stored_row(dir: &std::path::Path) -> Option<crate::directory::row::StoredRow> {
    let mut storage = crate::storage::SqliteStorage::open(&dir.join("object.db")).expect("reopens");
    crate::directory::row::current(&mut storage).expect("reads")
}

/// The row derives from the same state the call wrote, inside the
/// call's own transaction, so the two can never disagree.
#[tokio::test(flavor = "multi_thread")]
async fn a_call_that_wrote_derives_its_directory_row() {
    let source = r#"
        local Auction = object "Auction" {
            init = function(state)
                state.sql:exec("CREATE TABLE bids (amount INTEGER)")
            end,
            directory = {
                from = function(state)
                    return {
                        status = state.store:get("status") or "open",
                        high_bid = state.store:get("high_bid"),
                    }
                end,
                fields = {
                    status = f.string,
                    high_bid = f.integer,
                    tags = f.array(function(lot) return { "vintage", "rare" } end),
                },
            },
            bid = function(state, amount)
                state.sql:exec("INSERT INTO bids VALUES (?)", { amount })
                state.store:set("high_bid", amount)
            end,
        }
        on "fetch" (function() return { body = "ok" } end)
    "#;
    let dir = tempfile::tempdir().expect("tempdir");
    let handle = stored_object(source, dir.path()).await;

    handle
        .call(
            "__dispatch",
            serde_json::json!({
                "class": "Auction", "name": "lot42", "method": "bid",
                "args": [25], "chain": [],
            }),
        )
        .await
        .expect("bids");

    let row = stored_row(dir.path()).expect("the call derived a row");
    assert_eq!(row.rev, 1);
    assert_eq!(row.failed, None);
    use crate::directory::shape::Value;
    assert_eq!(
        row.fields,
        vec![
            ("high_bid".to_owned(), Value::Integer(25)),
            ("status".to_owned(), Value::Text("open".to_owned())),
            (
                "tags".to_owned(),
                Value::Array(vec![
                    Value::Text("vintage".to_owned()),
                    Value::Text("rare".to_owned()),
                ])
            ),
        ]
    );
}

/// The rule the whole feature rests on: a derived index may never
/// veto a business write. A throwing directory keeps the last good
/// row, marks the failure, and lets the call answer normally.
#[tokio::test(flavor = "multi_thread")]
async fn a_throwing_directory_never_fails_the_call() {
    let source = r#"
        local Ledger = object "Ledger" {
            init = function(state)
                state.sql:exec("CREATE TABLE entries (n INTEGER)")
            end,
            directory = {
                from = function(state)
                    local n = state.store:get("count")
                    if n and n > 1 then
                        error("the directory is buggy")
                    end
                    return { count = n }
                end,
                fields = { count = f.integer },
            },
            add = function(state, n)
                state.sql:exec("INSERT INTO entries VALUES (?)", { n })
                state.store:set("count", (state.store:get("count") or 0) + 1)
                return state.sql:query_one("SELECT count(*) AS c FROM entries").c
            end,
        }
        on "fetch" (function() return { body = "ok" } end)
    "#;
    let dir = tempfile::tempdir().expect("tempdir");
    let handle = stored_object(source, dir.path()).await;

    let call = |n: i64| {
        handle.call(
            "__dispatch",
            serde_json::json!({
                "class": "Ledger", "name": "main", "method": "add",
                "args": [n], "chain": [],
            }),
        )
    };

    assert_eq!(call(1).await.expect("first call answers"), 1);
    let good = stored_row(dir.path()).expect("a row exists");
    use crate::directory::shape::Value;
    assert_eq!(good.fields, vec![("count".to_owned(), Value::Integer(1))]);

    // The second call's directory throws. The business write must
    // still commit and answer.
    assert_eq!(
        call(2).await.expect("the call answers despite the throw"),
        2,
        "a derived index may never veto a business write"
    );

    let after = stored_row(dir.path()).expect("the row survives");
    assert_eq!(
        after.fields, good.fields,
        "a failed derivation keeps the last good row"
    );
    assert!(after.failed.is_some(), "the failure is marked, not silent");
    assert!(after.rev > good.rev, "a failure still advances the rev");

    // A third call proves the read-only window was cleared: writes
    // still work after a throwing derivation.
    assert_eq!(call(3).await.expect("writes still work"), 3);
}

/// The derivation cannot change the state it describes: both doors
/// (script sql and the store verbs) refuse, and refusing is a
/// contained failure rather than a broken call.
#[tokio::test(flavor = "multi_thread")]
async fn a_directory_that_writes_is_refused() {
    let source = r#"
        local Sneaky = object "Sneaky" {
            init = function(state)
                state.sql:exec("CREATE TABLE t (n INTEGER)")
            end,
            directory = {
                from = function(state)
                    state.sql:exec("INSERT INTO t VALUES (1)")
                    return { ok = true }
                end,
                fields = { ok = f.boolean },
            },
            touch = function(state)
                state.sql:exec("INSERT INTO t VALUES (0)")
                return state.sql:query_one("SELECT count(*) AS c FROM t").c
            end,
        }
        on "fetch" (function() return { body = "ok" } end)
    "#;
    let dir = tempfile::tempdir().expect("tempdir");
    let handle = stored_object(source, dir.path()).await;

    let answer = handle
        .call(
            "__dispatch",
            serde_json::json!({
                "class": "Sneaky", "name": "a", "method": "touch",
                "args": [], "chain": [],
            }),
        )
        .await
        .expect("the call answers");
    assert_eq!(answer, serde_json::json!(1), "only the method's row landed");

    let row = stored_row(dir.path()).expect("a marker exists");
    assert!(
        row.failed.is_some(),
        "a writing directory is a failed derivation"
    );
    assert!(row.fields.is_empty(), "and it stored no fields");
}

/// The destroying call's caller still hears its answer, the platform
/// hook runs exactly once, and everything after is a refusal.
#[tokio::test(flavor = "multi_thread")]
async fn destroy_answers_first_then_ends_the_object() {
    let source = r#"
        local Vault = object "Vault" {
            put = function(state, value)
                state.sql:exec("CREATE TABLE IF NOT EXISTS v (n INTEGER)")
                state.sql:exec("INSERT INTO v VALUES (?)", { value })
            end,
            close = function(state)
                local n = state.sql:query_one("SELECT count(*) AS n FROM v").n
                state:destroy()
                return n
            end,
        }
        on "fetch" (function() return { body = "ok" } end)
    "#;
    let runtime = runtime_with(source).await;
    let dir = tempfile::tempdir().expect("tempdir");
    let ran = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let counter = ran.clone();
    let destroy: DestroyFn = Arc::new(move || {
        let counter = counter.clone();
        Box::pin(async move {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        })
    });
    let handle = spawn_object_task(
        runtime,
        TaskOptions {
            storage: Some(
                crate::storage::SqliteStorage::open(&dir.path().join("vault.db")).expect("opens"),
            ),
            destroy: Some(destroy),
            ..Default::default()
        },
    );

    let dispatch = |method: &str| {
        serde_json::json!({
            "class": "Vault", "name": "a", "method": method,
            "args": [7], "chain": [],
        })
    };
    handle
        .call("__dispatch", dispatch("put"))
        .await
        .expect("puts");
    let answer = handle
        .call("__dispatch", dispatch("close"))
        .await
        .expect("the destroying call still answers");
    assert_eq!(answer, serde_json::json!(1));

    // The teardown runs after the answer; wait for the hook, then
    // every further call refuses.
    for _ in 0..100 {
        if ran.load(std::sync::atomic::Ordering::SeqCst) == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(ran.load(std::sync::atomic::Ordering::SeqCst), 1);
    let refused = handle.call("__dispatch", dispatch("put")).await;
    assert!(refused.is_err(), "a destroyed object refuses: {refused:?}");
}

/// The store face and sql ride one transaction: a method touching
/// both commits once, and a class with no migrations pays nothing
/// to keep small state.
#[tokio::test(flavor = "multi_thread")]
async fn the_store_face_rides_the_calls_own_transaction() {
    let source = r#"
        local Counter = object "Counter" {
            hit = function(state)
                local n = (state.store:get("count") or 0) + 1
                state.store:set("count", n)
                state.store:set("meta", { phase = "open", by = state.name })
                state.sql:exec("CREATE TABLE IF NOT EXISTS log (n INTEGER)")
                state.sql:exec("INSERT INTO log VALUES (?)", { n })
                return n
            end,
            peek = function(state)
                local page = state.store:list()
                local rows = state.sql:query_one("SELECT count(*) AS c FROM log").c
                return {
                    count = state.store:get("count"),
                    meta = state.store:get("meta"),
                    keys = #page.entries,
                    rows = rows,
                }
            end,
        }
        on "fetch" (function() return { body = "ok" } end)
    "#;
    let runtime = runtime_with(source).await;
    let dir = tempfile::tempdir().expect("tempdir");
    let handle = spawn_object_task(
        runtime,
        TaskOptions {
            storage: Some(
                crate::storage::SqliteStorage::open(&dir.path().join("counter.db")).expect("opens"),
            ),
            ..Default::default()
        },
    );

    let dispatch = |method: &str| {
        serde_json::json!({
            "class": "Counter", "name": "a", "method": method,
            "args": [], "chain": [],
        })
    };
    handle
        .call("__dispatch", dispatch("hit"))
        .await
        .expect("hits");
    let n = handle
        .call("__dispatch", dispatch("hit"))
        .await
        .expect("hits");
    assert_eq!(n, serde_json::json!(2));

    let peek = handle
        .call("__dispatch", dispatch("peek"))
        .await
        .expect("peeks");
    assert_eq!(peek["count"], serde_json::json!(2));
    assert_eq!(peek["meta"]["phase"], serde_json::json!("open"));
    assert_eq!(peek["keys"], serde_json::json!(2));
    assert_eq!(
        peek["rows"],
        serde_json::json!(2),
        "one transaction, both faces"
    );
}

/// A class whose schema comes from migration files applies them at
/// the instance's first touch, before init, and init then seeds. A
/// class without the key keeps init-owns-schema.
#[tokio::test(flavor = "multi_thread")]
async fn class_migrations_build_the_schema_before_init() {
    let source = r#"
        local Ledger = object "Ledger" {
            migrations = "migrations/Ledger",
            hooks = {
                init = function(state)
                    -- Seeding, not schema: the table already exists.
                    state.sql:exec("INSERT INTO entries (note) VALUES ('opened')")
                end,
            },
            notes = function(state)
                return state.sql:query("SELECT note FROM entries ORDER BY rowid")
            end,
        }
        on "fetch" (function() return { body = "ok" } end)
    "#;
    let runtime = runtime_with_files(&[
        ("main.lua", source),
        (
            "migrations/Ledger/0001_entries.sql",
            "CREATE TABLE entries (note TEXT);",
        ),
    ])
    .await;

    let dir = tempfile::tempdir().expect("tempdir");
    let handle = spawn_object_task(
        runtime,
        TaskOptions {
            storage: Some(
                crate::storage::SqliteStorage::open(&dir.path().join("ledger.db")).expect("opens"),
            ),
            ..Default::default()
        },
    );

    let notes = handle
        .call(
            "__dispatch",
            serde_json::json!({
                "class": "Ledger", "name": "house", "method": "notes",
                "args": [], "chain": [],
            }),
        )
        .await
        .expect("the migration ran before init, so init could seed");
    assert_eq!(
        notes[0]["note"], "opened",
        "schema from files, first row from init: {notes}"
    );
}

/// `state:method(...)` dispatches the class's own routable methods
/// directly, so sibling behavior needs no hoisted helper; hooks and
/// non-method keys stay invisible through the same gate handles use.
#[tokio::test(flavor = "multi_thread")]
async fn state_reaches_its_own_methods_directly() {
    let source = r#"
        local Counter = object "Counter" {
            publishes = { "ticks" },
            hooks = {
                init = function(state)
                    state.sql:exec("CREATE TABLE ticks (at INTEGER)")
                    -- A hook may use a sibling method too.
                    state:bump(0)
                end,
            },
            bump = function(state, at)
                state.sql:exec("INSERT INTO ticks (at) VALUES (?)", { at })
            end,
            twice = function(state)
                state:bump(1)
                state:bump(2)
                return {
                    count = state.sql:query_one("SELECT COUNT(*) AS n FROM ticks").n,
                    hooks_hidden = state.hooks == nil and state.init == nil,
                    contracts_hidden = state.publishes == nil,
                }
            end,
        }
        on "fetch" (function() return { body = "ok" } end)
    "#;
    let runtime = runtime_with(source).await;

    let dir = tempfile::tempdir().expect("tempdir");
    let handle = spawn_object_task(
        runtime,
        TaskOptions {
            storage: Some(
                crate::storage::SqliteStorage::open(&dir.path().join("counter.db")).expect("opens"),
            ),
            ..Default::default()
        },
    );

    let result = handle
        .call(
            "__dispatch",
            serde_json::json!({
                "class": "Counter", "name": "c", "method": "twice",
                "args": [], "chain": [],
            }),
        )
        .await
        .expect("sibling dispatch works from init and from a method");
    assert_eq!(result["count"], 3, "init's bump plus two more: {result}");
    assert_eq!(
        result["hooks_hidden"], true,
        "hooks stay out of the state surface: {result}"
    );
    assert_eq!(
        result["contracts_hidden"], true,
        "contract keys stay out of the state surface: {result}"
    );
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
                runtime_with("count = 0 function bump() count = count + 1 return count end").await,
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

        let response: serde_json::Value = runtime.from_value(response).expect("response converts");
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
                queues: vec![],
                workflows: vec![],
                workflow_steps: vec![],
                publishes: vec![],
                lifecycle: vec![],
                connections: vec![],
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
        KvServiceClient::new(crate::plain_grpc(channel)),
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
    // A distant backstop, so the work ceiling is what stops the
    // loop rather than the clock racing it on a busy machine.
    let handle = spawn_object_task(
        runtime,
        TaskOptions {
            call_budget: Some(30),
            ..Default::default()
        },
    );

    let started = std::time::Instant::now();
    let error = handle
        .call("spin", serde_json::Value::Null)
        .await
        .expect_err("the runaway must be stopped");
    assert!(matches!(error, ObjectError::Call(_)), "{error}");
    // Work catches this, and says so, rather than sending the
    // author to look at latency.
    assert!(
        error.to_string().contains("too much work"),
        "the work meter should stop this, not the clock: {error}"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(30),
        "stopped by work in {:?}, long before the backstop",
        started.elapsed()
    );

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
        let response: serde_json::Value = runtime.from_value(response).expect("response converts");
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
async fn the_alarm_mirror_sees_every_cell_change() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("keeper.db");

    let seen: Arc<std::sync::Mutex<Vec<Option<i64>>>> = Arc::default();
    let recorder: AlarmSync = {
        let seen = seen.clone();
        Arc::new(move |due_ms| seen.lock().expect("no poison").push(due_ms))
    };

    let handle = spawn_object_task(
        runtime_with(LIFECYCLE_SOURCE).await,
        TaskOptions {
            storage: Some(crate::storage::SqliteStorage::open(&path).expect("opens")),
            alarm_sync: Some(recorder),
            ..Default::default()
        },
    );

    handle
        .call(
            "__dispatch",
            keeper_call("poke", serde_json::json!(["150ms"])),
        )
        .await
        .expect("poke");
    tokio::time::sleep(std::time::Duration::from_millis(900)).await;

    let journal = seen.lock().expect("no poison").clone();
    // Spawn syncs the (empty) file truth, the arm mirrors its due
    // time, the fire mirrors the clear: heal, arm, clear.
    assert_eq!(journal.len(), 3, "{journal:?}");
    assert_eq!(journal[0], None, "spawn syncs the file truth");
    assert!(journal[1].is_some(), "the arm carries its due time");
    assert_eq!(journal[2], None, "the fire clears the mirror");
    drop(handle);

    // A respawn over a file that still holds an alarm re-mirrors it:
    // the heal that makes lost async writes safe.
    let second = spawn_object_task(
        runtime_with(LIFECYCLE_SOURCE).await,
        TaskOptions {
            storage: Some(crate::storage::SqliteStorage::open(&path).expect("opens")),
            alarm_sync: Some({
                let seen = seen.clone();
                Arc::new(move |due_ms| seen.lock().expect("no poison").push(due_ms))
            }),
            ..Default::default()
        },
    );
    second
        .call(
            "__dispatch",
            keeper_call("poke", serde_json::json!(["10s"])),
        )
        .await
        .expect("poke");
    drop(second);

    let third = spawn_object_task(
        runtime_with(LIFECYCLE_SOURCE).await,
        TaskOptions {
            storage: Some(crate::storage::SqliteStorage::open(&path).expect("opens")),
            alarm_sync: Some({
                let seen = seen.clone();
                Arc::new(move |due_ms| seen.lock().expect("no poison").push(due_ms))
            }),
            ..Default::default()
        },
    );
    // Force the spawn to complete before reading the journal.
    third
        .call("__dispatch", keeper_call("alarms", serde_json::json!([])))
        .await
        .expect("alarms");

    let journal = seen.lock().expect("no poison").clone();
    assert!(
        journal.last().expect("entries").is_some(),
        "a respawn over an armed file must re-mirror the alarm: {journal:?}"
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

/// A database declared without a migrations directory is manual: the
/// platform applies nothing, even when files sit in the conventional
/// `migrations/<name>/` location.
#[tokio::test(flavor = "multi_thread")]
async fn a_database_without_a_declared_directory_applies_nothing() {
    let runtime = runtime_with_files(&[
        ("main.lua", r#"local db = database "notes""#),
        (
            "migrations/notes/0001_init.sql",
            "CREATE TABLE visits (at INTEGER);",
        ),
    ])
    .await;

    let dir = tempfile::tempdir().expect("tempdir");
    let handle = spawn_object_task(
        runtime,
        TaskOptions {
            storage: Some(
                crate::storage::SqliteStorage::open(&dir.path().join("notes.db")).expect("opens"),
            ),
            ..Default::default()
        },
    );
    let error = handle
        .call(
            "__dispatch",
            serde_json::json!({
                "class": "__database", "name": "notes", "method": "exec",
                "args": ["INSERT INTO visits VALUES (1)"],
            }),
        )
        .await
        .expect_err("no schema was applied, so the table cannot exist");
    assert!(
        error.to_string().contains("visits"),
        "the failure must be the missing table, got: {error}"
    );
}

/// Migrations apply at first touch, exactly once per database, and a
/// respawn over the same file never reapplies them (the CREATE would
/// fail if it did).
#[tokio::test(flavor = "multi_thread")]
async fn migrations_apply_once_at_first_touch() {
    const MAIN: &str = r#"local db = database "main" { migrations = "migrations/main" }"#;
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
                crate::storage::SqliteStorage::open(&dir.path().join("ledger.db")).expect("opens"),
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
                crate::storage::SqliteStorage::open(&dir.path().join("cron.db")).expect("opens"),
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

/// The queue substrate end to end: send enqueues, the alarm loop
/// delivers to the `on "queue:<name>"` listener, and the payload
/// survives the json round trip through sqlite.
#[tokio::test(flavor = "multi_thread")]
async fn a_queue_delivers_sent_messages_to_the_listener() {
    const SOURCE: &str = r#"
        got = nil
        on "queue:jobs" (function(message)
            sleep_ms(5)
            got = message
        end)
        function get_got() return got end
    "#;

    let dir = tempfile::tempdir().expect("tempdir");
    let handle = spawn_object_task(
        runtime_with(SOURCE).await,
        TaskOptions {
            storage: Some(
                crate::storage::SqliteStorage::open(&dir.path().join("q.db")).expect("opens"),
            ),
            ..Default::default()
        },
    );

    handle
        .call(
            "__dispatch",
            serde_json::json!({
                "class": "__queue", "name": "jobs", "method": "send",
                "args": [{"kind": "render", "frame": 17}],
            }),
        )
        .await
        .expect("send enqueues");

    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let got = handle
        .call("get_got", serde_json::Value::Null)
        .await
        .expect("read back");
    assert_eq!(got["kind"], "render", "payload round-trips: {got}");
    assert_eq!(got["frame"], 17);
}

/// A refused delivery retries with backoff and succeeds on the second
/// attempt; nothing is lost and nothing dead-letters.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_delivery_retries_until_the_handler_accepts() {
    const SOURCE: &str = r#"
        tries = 0
        on "queue:jobs" (function(message)
            tries = tries + 1
            if tries < 2 then error("not yet") end
        end)
        function get_tries() return tries end
    "#;

    let dir = tempfile::tempdir().expect("tempdir");
    let handle = spawn_object_task(
        runtime_with(SOURCE).await,
        TaskOptions {
            storage: Some(
                crate::storage::SqliteStorage::open(&dir.path().join("q.db")).expect("opens"),
            ),
            // Compressed backoff: production waits seconds, the test
            // needs the retry inside its own window.
            queue: crate::platform::queue::QueuePolicy {
                backoff_base_ms: 20,
                ..Default::default()
            },
            ..Default::default()
        },
    );

    handle
        .call(
            "__dispatch",
            serde_json::json!({
                "class": "__queue", "name": "jobs", "method": "send", "args": ["once"],
            }),
        )
        .await
        .expect("send enqueues");

    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    let tries = handle
        .call("get_tries", serde_json::Value::Null)
        .await
        .expect("tries read");
    assert_eq!(tries.as_i64(), Some(2), "one refusal, one delivery");

    let stats = handle
        .call(
            "__dispatch",
            serde_json::json!({
                "class": "__queue", "name": "jobs", "method": "stats", "args": [],
            }),
        )
        .await
        .expect("stats");
    assert_eq!(stats["depth"], 0, "the retried message was consumed");
    assert_eq!(stats["dead_letters"], 0);
}

/// A poison message exhausts its attempts and lands in the dead-letter
/// table instead of blocking the queue forever.
#[tokio::test(flavor = "multi_thread")]
async fn a_poison_message_dead_letters_after_max_attempts() {
    const SOURCE: &str = r#"
        on "queue:jobs" (function(message)
            error("always refuses")
        end)
    "#;

    let dir = tempfile::tempdir().expect("tempdir");
    let handle = spawn_object_task(
        runtime_with(SOURCE).await,
        TaskOptions {
            storage: Some(
                crate::storage::SqliteStorage::open(&dir.path().join("q.db")).expect("opens"),
            ),
            // Compressed backoff so all five attempts fit the test.
            queue: crate::platform::queue::QueuePolicy {
                backoff_base_ms: 5,
                ..Default::default()
            },
            ..Default::default()
        },
    );

    handle
        .call(
            "__dispatch",
            serde_json::json!({
                "class": "__queue", "name": "jobs", "method": "send", "args": ["poison"],
            }),
        )
        .await
        .expect("send enqueues");

    tokio::time::sleep(std::time::Duration::from_millis(900)).await;
    let stats = handle
        .call(
            "__dispatch",
            serde_json::json!({
                "class": "__queue", "name": "jobs", "method": "stats", "args": [],
            }),
        )
        .await
        .expect("stats");
    assert_eq!(stats["depth"], 0, "the poison message left the queue");
    assert_eq!(
        stats["dead_letters"], 1,
        "and landed in the dead-letter table"
    );
}

/// The inspector's contract: the journal carries message ids,
/// producers and per-attempt error text; dead letters requeue through
/// retry_dead and deliver; drop discards for good.
#[tokio::test(flavor = "multi_thread")]
async fn the_journal_carries_producers_and_dead_letters_requeue() {
    const SOURCE: &str = r#"
        accept = false
        got = nil
        on "queue:jobs" (function(message)
            if not accept then error("not ready: refusing") end
            got = message
        end)
        function allow() accept = true end
        function deny() accept = false end
        function get_got() return got end
    "#;

    let dir = tempfile::tempdir().expect("tempdir");
    let handle = spawn_object_task(
        runtime_with(SOURCE).await,
        TaskOptions {
            storage: Some(
                crate::storage::SqliteStorage::open(&dir.path().join("q.db")).expect("opens"),
            ),
            // Two attempts at a compressed backoff: dead fast.
            queue: crate::platform::queue::QueuePolicy {
                max_attempts: 2,
                backoff_base_ms: 5,
            },
            ..Default::default()
        },
    );
    let dispatch = |method: &str, args: serde_json::Value| {
        serde_json::json!({
            "class": "__queue", "name": "jobs", "method": method, "args": args,
            "caller": { "script": "todo-api", "revision": "c47d1b90" },
        })
    };

    handle
        .call(
            "__dispatch",
            dispatch("send", serde_json::json!([{ "n": 1 }])),
        )
        .await
        .expect("send enqueues");
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // The journal: enqueued names the producer, the dead letter its
    // last error.
    let events = handle
        .call("__dispatch", dispatch("events", serde_json::json!([0])))
        .await
        .expect("events read");
    let events = events.as_array().expect("events are rows");
    let enqueued = events
        .iter()
        .find(|event| event["kind"] == "enqueued")
        .expect("the enqueue was journaled");
    assert_eq!(enqueued["detail"]["producer_script"], "todo-api");
    assert_eq!(enqueued["detail"]["producer_revision"], "c47d1b90");
    let dead = events
        .iter()
        .find(|event| event["kind"] == "dead-lettered")
        .expect("the dead letter was journaled");
    assert!(
        dead["detail"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("not ready")),
        "the attempt's error text is recorded: {dead}"
    );

    // Requeue and let the now-willing handler consume it.
    handle
        .call("allow", serde_json::Value::Null)
        .await
        .expect("allow");
    let requeued = handle
        .call("__dispatch", dispatch("retry_dead", serde_json::json!([])))
        .await
        .expect("retry_dead");
    assert_eq!(requeued.as_i64(), Some(1), "one dead letter requeued");
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;

    let got = handle
        .call("get_got", serde_json::Value::Null)
        .await
        .expect("read back");
    assert_eq!(got["n"], 1, "the requeued message delivered: {got}");
    let stats = handle
        .call("__dispatch", dispatch("stats", serde_json::json!([])))
        .await
        .expect("stats");
    assert_eq!(stats["depth"], 0);
    assert_eq!(stats["dead_letters"], 0);

    // A second poison message dies again; drop discards it for good.
    handle
        .call("deny", serde_json::Value::Null)
        .await
        .expect("deny");
    handle
        .call(
            "__dispatch",
            dispatch("send", serde_json::json!(["poison"])),
        )
        .await
        .expect("send enqueues");
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let messages = handle
        .call("__dispatch", dispatch("messages", serde_json::json!([])))
        .await
        .expect("messages read");
    let dead_row = messages
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["state"] == "dead"))
        .expect("the poison row is listed dead")
        .clone();
    let dropped = handle
        .call(
            "__dispatch",
            dispatch("drop_message", serde_json::json!([dead_row["id"]])),
        )
        .await
        .expect("drop");
    assert_eq!(dropped, serde_json::Value::Bool(true));
    let stats = handle
        .call("__dispatch", dispatch("stats", serde_json::json!([])))
        .await
        .expect("stats");
    assert_eq!(stats["dead_letters"], 0, "the drop removed it");
}

/// Message ids are never reused: a delivered message's id stays
/// retired, so a journal id names exactly one message ever.
#[tokio::test(flavor = "multi_thread")]
async fn message_ids_never_reuse_after_delivery() {
    const SOURCE: &str = r#"
        on "queue:jobs" (function(message) end)
    "#;

    let dir = tempfile::tempdir().expect("tempdir");
    let handle = spawn_object_task(
        runtime_with(SOURCE).await,
        TaskOptions {
            storage: Some(
                crate::storage::SqliteStorage::open(&dir.path().join("q.db")).expect("opens"),
            ),
            ..Default::default()
        },
    );
    let dispatch = |method: &str, args: serde_json::Value| {
        serde_json::json!({
            "class": "__queue", "name": "jobs", "method": method, "args": args,
        })
    };

    handle
        .call("__dispatch", dispatch("send", serde_json::json!(["first"])))
        .await
        .expect("send");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    handle
        .call(
            "__dispatch",
            dispatch("send", serde_json::json!(["second"])),
        )
        .await
        .expect("send");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let events = handle
        .call("__dispatch", dispatch("events", serde_json::json!([0])))
        .await
        .expect("events read");
    let ids: Vec<i64> = events
        .as_array()
        .expect("rows")
        .iter()
        .filter(|event| event["kind"] == "enqueued")
        .filter_map(|event| event["detail"]["id"].as_i64())
        .collect();
    assert_eq!(ids.len(), 2, "both enqueues journaled: {events}");
    assert!(
        ids[1] > ids[0],
        "the second message must not reuse the delivered id: {ids:?}"
    );
}

/// A version 1 queue file (rowid ids that could reuse) is carried to
/// version 2 on first touch: rows survive, and new ids resume past
/// the highest existing one.
#[tokio::test(flavor = "multi_thread")]
async fn a_v1_queue_file_migrates_and_keeps_its_rows() {
    const SOURCE: &str = r#"
        on "queue:jobs" (function(message) error("hold the queue") end)
    "#;

    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("q.db");
    {
        // Hand-build the v1 schema: plain rowid ids, version cell 1.
        let mut storage = crate::storage::SqliteStorage::open(&file).expect("opens");
        storage
            .platform()
            .execute_batch(
                "CREATE TABLE __actias_queue_messages (
                    id INTEGER PRIMARY KEY,
                    payload TEXT NOT NULL,
                    attempts INTEGER NOT NULL DEFAULT 0,
                    next_at INTEGER NOT NULL,
                    enqueued_at INTEGER NOT NULL
                );
                CREATE TABLE __actias_queue_dead (
                    id INTEGER, payload TEXT, attempts INTEGER,
                    enqueued_at INTEGER, died_at INTEGER
                );
                INSERT INTO __actias_queue_messages VALUES (7, '\"held\"', 0, 9999999999999, 1);",
            )
            .expect("v1 schema builds");
        storage.set_schema_version(1).expect("stamps v1");
    }

    let handle = spawn_object_task(
        runtime_with(SOURCE).await,
        TaskOptions {
            storage: Some(crate::storage::SqliteStorage::open(&file).expect("reopens")),
            ..Default::default()
        },
    );
    let dispatch = |method: &str, args: serde_json::Value| {
        serde_json::json!({
            "class": "__queue", "name": "jobs", "method": method, "args": args,
        })
    };

    // First touch migrates; the held row survives it.
    let rows = handle
        .call("__dispatch", dispatch("messages", serde_json::json!([])))
        .await
        .expect("messages read");
    assert_eq!(rows[0]["id"], 7, "the v1 row carried over: {rows}");

    // New ids resume past the carried-over ones.
    handle
        .call("__dispatch", dispatch("send", serde_json::json!(["fresh"])))
        .await
        .expect("send");
    let rows = handle
        .call("__dispatch", dispatch("messages", serde_json::json!([])))
        .await
        .expect("messages read");
    let ids: Vec<i64> = rows
        .as_array()
        .expect("rows")
        .iter()
        .filter_map(|row| row["id"].as_i64())
        .collect();
    assert!(ids.contains(&7), "old row still listed: {ids:?}");
    assert!(
        ids.iter().any(|id| *id > 7),
        "new ids resume past the old ones: {ids:?}"
    );
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
/// Not an assertion, a measurement: what guest work costs, which is
/// what a work ceiling would be set from. Run with
/// `--ignored --nocapture` to re-derive it on other hardware.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn tick_rate() {
    let runtime = runtime_with(
        r#"
        function spin() while true do end end
        function work() local s = 0 for i = 1, 200000 do s = s + i end return s end
        function trivial() return 1 end
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
    for name in ["trivial", "work"] {
        let before = std::time::Instant::now();
        let out = handle.call(name, serde_json::Value::Null).await;
        println!("PROBE {name}: {:?} ok={}", before.elapsed(), out.is_ok());
    }
    let before = std::time::Instant::now();
    let err = handle.call("spin", serde_json::Value::Null).await;
    println!(
        "PROBE spin stopped after {:?}: {}",
        before.elapsed(),
        err.err().map(|e| e.to_string()).unwrap_or_default()
    );
}

/// Depth is the other unbounded shape: no cycle, but every hop is a
/// mailbox and possibly a network forward.
#[test]
fn a_call_chain_is_refused_once_it_runs_away() {
    let shallow: Vec<String> = (0..5).map(|n| format!("p/C/{n}")).collect();
    extend_call_chain(&shallow, "p/C/next").expect("ordinary nesting is fine");

    let deep: Vec<String> = (0..MAX_CALL_DEPTH).map(|n| format!("p/C/{n}")).collect();
    let refused =
        extend_call_chain(&deep, "p/C/one-more").expect_err("past the depth limit it is a runaway");
    assert!(refused.contains("past the limit"), "{refused}");

    // The cycle refusal is unchanged and still takes precedence.
    let cyclic = vec!["p/C/a".to_owned(), "p/C/b".to_owned()];
    let cycle = extend_call_chain(&cyclic, "p/C/a").expect_err("a cycle deadlocks");
    assert!(cycle.contains("Reentrant"), "{cycle}");
}

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
                observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Box::pin(async move { Ok(()) })
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

/// A reply that wrote nothing waits when the object has writes in the
/// air, and pays nothing once it is settled: the read gate asks the
/// hook, and the hook decides.
#[tokio::test(flavor = "multi_thread")]
async fn a_read_waits_while_earlier_writes_are_in_flight() {
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

    let unsettled = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let waited = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let dir = tempfile::tempdir().expect("tempdir");
    let handle = spawn_object_task(
        runtime_with(SOURCE).await,
        TaskOptions {
            storage: Some(
                crate::storage::SqliteStorage::open(&dir.path().join("k.db")).expect("opens"),
            ),
            after_write: Some(Arc::new(|| Box::pin(async { Ok(()) }))),
            after_read: Some({
                let unsettled = unsettled.clone();
                let asked = asked.clone();
                let waited = waited.clone();
                Arc::new(move || {
                    asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if !unsettled.load(std::sync::atomic::Ordering::SeqCst) {
                        return None;
                    }
                    let waited = waited.clone();
                    Some(Box::pin(async move {
                        waited.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        Ok(())
                    }))
                })
            }),
            ..Default::default()
        },
    );

    handle.call("__dispatch", call("put")).await.expect("put");
    // A write's own gate answers it; the read gate is not asked.
    assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 0);

    // Unsettled: the read asks and waits.
    handle.call("__dispatch", call("peek")).await.expect("peek");
    assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(waited.load(std::sync::atomic::Ordering::SeqCst), 1);

    // Settled: the read asks and is answered at once.
    unsettled.store(false, std::sync::atomic::Ordering::SeqCst);
    handle.call("__dispatch", call("peek")).await.expect("peek");
    assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(waited.load(std::sync::atomic::Ordering::SeqCst), 1);
}

/// A write is answered only after its gate resolves, and a gate that
/// cannot confirm turns the answer into an unknown outcome rather
/// than a success or a method failure.
#[tokio::test(flavor = "multi_thread")]
async fn a_write_waits_for_its_gate_and_reports_an_unconfirmed_one() {
    const SOURCE: &str = r#"
        local Keeper = object "Keeper" {
            init = function(state)
                state.sql:exec("CREATE TABLE t (n INTEGER)")
            end,
            put = function(state)
                state.sql:exec("INSERT INTO t VALUES (1)")
            end,
        }
    "#;
    let call = serde_json::json!({ "class": "Keeper", "method": "put", "args": [] });
    let dir = tempfile::tempdir().expect("tempdir");

    // The gate the test drives by hand: the first call's answer must
    // not appear until this is released.
    let (release, released) = tokio::sync::watch::channel(false);
    let confirm = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let gate_confirms = confirm.clone();
    // Counted at dispatch, so it says how many calls ran, whatever
    // their answers are doing.
    let dispatched = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let ran = dispatched.clone();

    let handle = spawn_object_task(
        runtime_with(SOURCE).await,
        TaskOptions {
            storage: Some(
                crate::storage::SqliteStorage::open(&dir.path().join("k.db")).expect("opens"),
            ),
            after_write: Some(Arc::new(move || {
                let mut released = released.clone();
                let confirms = gate_confirms.clone();
                ran.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Box::pin(async move {
                    while !*released.borrow_and_update() {
                        if released.changed().await.is_err() {
                            break;
                        }
                    }
                    if confirms.load(std::sync::atomic::Ordering::SeqCst) {
                        Ok(())
                    } else {
                        Err("the store never answered".to_owned())
                    }
                })
            })),
            ..Default::default()
        },
    );

    let answering: Vec<_> = (0..2)
        .map(|_| {
            let handle = handle.clone();
            let call = call.clone();
            tokio::spawn(async move { handle.call("__dispatch", call).await })
        })
        .collect();

    // Held: both writes committed, but nothing may be acknowledged
    // until the gate says the frames left the node. Both ran while
    // held, which is the property that keeps the gate a latency cost
    // and lets a burst share one flight: the input gate ends at the
    // commit, not at the acknowledgment.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        answering.iter().all(|task| !task.is_finished()),
        "an ungated write was answered"
    );
    assert_eq!(
        dispatched.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "a held answer must not hold the mailbox"
    );

    release.send_replace(true);
    for task in answering {
        task.await
            .expect("the answering task")
            .expect("the write is answered once its gate resolves");
    }

    // A gate that gives up makes the outcome unknown; the method
    // itself never failed, so ObjectError::Call would be a lie.
    confirm.store(false, std::sync::atomic::Ordering::SeqCst);
    let unknown = handle
        .call("__dispatch", call)
        .await
        .expect_err("an unconfirmed write is not a success");
    assert!(matches!(unknown, ObjectError::NotDurable(_)), "{unknown:?}");
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
