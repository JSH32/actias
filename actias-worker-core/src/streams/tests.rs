//! The stream tests.

use super::{
    ConnectionForwarder, Edge, LocalNode, NodeInbox, PublisherIdentity, ReceiveForwarder,
    ReceiveReport, head_seq, list_edges,
};
use crate::extensions::objects::{ObjectRouter, ObjectTarget};
use crate::objects::{ObjectHandle, TaskOptions, spawn_object_task};
use crate::proto::bundle::{Bundle, File};
use crate::proto::kv_service::kv_service_client::KvServiceClient;
use crate::proto::script_service::{Revision, Script};
use crate::runtime::{ActiasRuntime, PreparedRevision};
use std::sync::Arc;

/// A hub gating on membership and a reader recording deliveries:
/// the whole S1 surface in one fixture, hooks table and callable
/// handles included.
const SOURCE: &str = r#"
    local Hub
    local Reader

    Hub = object "Hub" {
        publishes = { "news", "noise", private = "self", plaza = "public" },
        hooks = {
            init = function(state)
                state.sql:exec("CREATE TABLE members (user TEXT PRIMARY KEY)")
            end,
            follow = function(state, topic, follower)
                return follower:is(Reader) and state.sql:query_one(
                    "SELECT 1 FROM members WHERE user = ?", { follower.name }) ~= nil
            end,
        },
        enroll = function(state, user)
            state.sql:exec("INSERT OR IGNORE INTO members VALUES (?)", { user })
        end,
        post = function(state, kind, text)
            state:publish("news", { kind = kind, text = text })
        end,
        blast = function(state)
            state:publish("noise", { kind = "static", text = "..." })
        end,
        leak = function(state)
            state:publish("secrets", { oops = true })
        end,
        kick = function(state, user)
            state.sql:exec("DELETE FROM members WHERE user = ?", { user })
            state:drop_followers(Reader(user))
        end,
        audience = function(state)
            return #state:followers("news")
        end,
        -- Pure-Luau probe, no async anywhere: can a native yield
        -- cross a generic-for iterator call?
        forin_pure_yield = function(state)
            local co = coroutine.create(function()
                for v in function() return coroutine.yield("ask") end do
                    return v
                end
            end)
            local ok1, ask = coroutine.resume(co)
            local ok2, got = coroutine.resume(co, "answer")
            return { ok1 = ok1, ask = tostring(ask), ok2 = ok2, got = tostring(got) }
        end,
    }

    local function record(state, event)
        if flaky_mode and state.name == "flaky" and not state.tripped then
            state.tripped = true
            error("transient outage")
        end
        state.sql:exec("INSERT INTO seen VALUES (?, ?, ?, ?)",
            { event.topic, event.data.kind, event.from.id, event.data.text })
    end

    Reader = object "Reader" {
        receives = {
            ["Hub:news"] = record,
            ["Hub:private"] = record,
        },
        hooks = {
            init = function(state)
                state.sql:exec(
                    "CREATE TABLE seen (topic TEXT, kind TEXT, from_id TEXT, text TEXT)")
            end,
        },
        join = function(state, hub, kind)
            local filter = nil
            if kind then filter = { kind = kind } end
            state:follow(Hub(hub), "news", filter)
        end,
        spy = function(state, hub)
            state:follow(Hub(hub), "private")
        end,
        eavesdrop = function(state, hub)
            -- Follows a stream this class declares NO receives entry
            -- for; the checker refuses this, the runtime discards.
            state:follow(Hub(hub), "noise")
        end,
        leave = function(state, hub)
            state:unfollow(Hub(hub), "news")
        end,
        seen = function(state)
            return state.sql:query("SELECT topic, kind, from_id, text FROM seen ORDER BY rowid")
        end,
        -- pcall around cross-object calls, the natural guard style:
        -- works only because pcall is Luau's native (yieldable) one.
        guarded = function(state)
            local ok, value = pcall(function()
                return Hub("town"):audience()
            end)
            local bad, why = pcall(function()
                return Hub("town"):leak()
            end)
            return { ok = ok, value = value, bad = bad, why = tostring(why) }
        end,
    }

    -- The one-stream socket with a declared event handler: the app
    -- chose its own wire shape. Omitting the handler forwards the
    -- platform envelope instead (Sticky is that case).
    local Forwarder = connection "Forwarder" {
        open = function(conn)
            conn:follow(Hub("town"), "news")
        end,
        event = function(conn, event)
            conn:send({ topic = event.topic,
                        from = event.from.id,
                        kind = event.data.kind })
        end,
    }

    -- Session state that must outlive the vm: the hibernation
    -- tests count frames across a drop. The forward policy rides
    -- along, so the same class also proves a delivery reaches the
    -- wire without building a vm to carry it.
    local Sticky = connection "Sticky" {
        event = "forward",
        frame = function(conn, data)
            conn.state.n = (conn.state.n or 0) + 1
            conn:send({ kind = "count", n = conn.state.n })
        end,
    }

    -- The heartbeat: ticks write the wire, a deliberately slow
    -- first tick makes the second one carry a missed count.
    local Beat = connection "Beat" {
        timer = { every = "1s", run = function(conn, missed)
            conn.state.ticks = (conn.state.ticks or 0) + 1
            conn:send({ kind = "tick", n = conn.state.ticks, missed = missed })
            if conn.state.slow == nil then
                conn.state.slow = true
                -- Slow by waiting, the way a real handler is slow.
                -- Burning cpu here would (correctly) hit the work
                -- budget instead of testing coalescing.
                test_slow(2400)
            end
        end },
    }

    local Echo = connection "Echo" {
        open = function(conn)
            conn:follow(Hub("town"), "news")
        end,
        event = function(conn, event)
            conn:send({ kind = "event", topic = event.topic,
                        from = event.from.id })
        end,
        frame = function(conn, data)
            conn:send({ kind = "frame", echo = data.hello })
            conn:close()
        end,
    }

    on "fetch" (function(request)
        -- The live landmine's exact shape: a fetch handler
        -- pcall-guarding cross-object calls; both arms must work.
        if request.guard_probe then
            local ok, value = pcall(function()
                return Hub("town"):audience()
            end)
            local bad, why = pcall(function()
                return Hub("town"):leak()
            end)
            return { ok = ok, value = value, bad = bad, why = tostring(why) }
        end
        -- The upgrade shape production uses: a declared class and
        -- an identity minted from an instance handle.
        if request.upgrade and request.wants_forward then
            return request:upgrade(Forwarder, Reader("ada"))
        end
        if request.upgrade and request.wants_sticky then
            return request:upgrade(Sticky, Reader("ada"))
        end
        if request.upgrade and request.wants_beat then
            return request:upgrade(Beat, Reader("ada"))
        end
        if request.upgrade then
            return request:upgrade(Echo, Reader("ada"))
        end
        return { body = "ok" }
    end)
"#;

/// A host function the connection tests use to be slow without
/// spending cpu: real handlers are slow because they wait, and the
/// work budget is meant to stop the ones that are slow because they
/// compute.
fn register_test_slow(runtime: &ActiasRuntime) {
    let slow = runtime
        .create_async_function(|_, ms: u64| async move {
            tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
            Ok(())
        })
        .expect("host fn builds");
    runtime
        .globals()
        .set("test_slow", slow)
        .expect("global sets");
}

async fn vm(source: &str, flaky: bool) -> ActiasRuntime {
    let source = if flaky {
        format!("flaky_mode = true\n{source}")
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
    let channel = tonic::transport::Channel::from_static("http://127.0.0.1:1").connect_lazy();
    ActiasRuntime::new(
        prepared,
        KvServiceClient::new(crate::plain_grpc(channel)),
        crate::egress::EgressClient::new(crate::egress::EgressPolicy::new([], false))
            .expect("egress builds"),
        None,
        None,
        None,
    )
    .await
    .expect("runtime builds")
}

/// The in-test placement: one vm per identity, all sharing one
/// router, so follows and deliveries route for real.
fn town_router(dir: std::path::PathBuf, flaky: bool) -> ObjectRouter {
    town_router_with(dir, flaky, Arc::default(), None, None)
}

/// Same placement with a shared connection registry, so connection
/// edges deliver into test-held inboxes.
fn town_router_with(
    dir: std::path::PathBuf,
    flaky: bool,
    registry: Arc<crate::connections::ConnectionRegistry>,
    fanout: Option<(String, ConnectionForwarder)>,
    durable: Option<(PublisherIdentity, ReceiveForwarder)>,
) -> ObjectRouter {
    type Registry = Arc<tokio::sync::Mutex<std::collections::HashMap<String, ObjectHandle>>>;
    let registry_map: Registry = Arc::default();
    let cell: Arc<std::sync::OnceLock<ObjectRouter>> = Arc::new(std::sync::OnceLock::new());

    let router_cell = cell.clone();
    let connections = registry;
    let router: ObjectRouter = Arc::new(move |target: ObjectTarget| {
        let registry = registry_map.clone();
        let cell = router_cell.clone();
        let dir = dir.clone();
        let connections = connections.clone();
        let fanout = fanout.clone();
        let durable = durable.clone();
        Box::pin(async move {
            let key = format!("{}/{}", target.class, target.name);
            let handle = {
                let mut map = registry.lock().await;
                if let Some(handle) = map.get(&key) {
                    handle.clone()
                } else {
                    let runtime = vm(SOURCE, flaky).await;
                    let router = cell.get().expect("router installed").clone();
                    runtime.set_app_data::<ObjectRouter>(router);
                    runtime.set_app_data::<Arc<crate::connections::ConnectionRegistry>>(
                        connections.clone(),
                    );
                    if let Some((node, forwarder)) = fanout.clone() {
                        runtime.set_app_data(LocalNode(node));
                        runtime.set_app_data::<ConnectionForwarder>(forwarder);
                    }
                    if let Some((identity, forwarder)) = durable.clone() {
                        runtime.set_app_data(identity);
                        runtime.set_app_data::<ReceiveForwarder>(forwarder);
                    }
                    let file = dir.join(format!("{}.db", key.replace(['/', ':'], "_")));
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
                        "chain": target.chain.iter().chain([&key].into_iter().map(|k| &**k).map(str::to_owned).collect::<Vec<_>>().iter()).cloned().collect::<Vec<String>>(),
                    }),
                )
                .await
                .map_err(|e| e.to_string())
        })
    });
    cell.set(router.clone()).ok();
    router
}

async fn call(
    router: &ObjectRouter,
    class: &str,
    name: &str,
    method: &str,
    args: Vec<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    router(ObjectTarget {
        class: class.to_owned(),
        name: name.to_owned(),
        method: method.to_owned(),
        arguments: args,
        chain: Vec::new(),
        caller: None,
    })
    .await
}

async fn seen_rows(router: &ObjectRouter, reader: &str) -> Vec<serde_json::Value> {
    call(router, "Reader", reader, "seen", vec![])
        .await
        .expect("seen answers")
        .as_array()
        .cloned()
        .unwrap_or_default()
}

async fn wait_for<F: Fn(&[serde_json::Value]) -> bool>(
    router: &ObjectRouter,
    reader: &str,
    good: F,
) -> Vec<serde_json::Value> {
    for _ in 0..80 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let rows = seen_rows(router, reader).await;
        if good(&rows) {
            return rows;
        }
    }
    seen_rows(router, reader).await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_contract_without_the_topic_refuses_publish() {
    use crate::proto::script_service::{Capabilities, ScriptConfig};

    // The bundle's class table declares publishes, but the stored
    // contract does not: the tamper case, refused loudly.
    let revision = Revision {
        bundle: Some(Bundle {
            entry_point: "main.lua".to_owned(),
            files: vec![File {
                file_path: "main.lua".to_owned(),
                content: SOURCE.as_bytes().to_vec(),
                ..Default::default()
            }],
        }),
        script_config: Some(ScriptConfig {
            id: "test".to_owned(),
            entry_point: "main.lua".to_owned(),
            includes: vec![],
            ignore: vec![],
            capabilities: Some(Capabilities {
                kv: vec![],
                events: vec!["fetch".to_owned()],
                secrets: vec![],
                objects: vec!["Hub".to_owned(), "Reader".to_owned()],
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
    let runtime = ActiasRuntime::new(
        prepared,
        KvServiceClient::new(crate::plain_grpc(channel)),
        crate::egress::EgressClient::new(crate::egress::EgressPolicy::new([], false))
            .expect("egress builds"),
        None,
        None,
        None,
    )
    .await
    .expect("runtime builds");

    let dir = tempfile::tempdir().expect("tempdir");
    let handle = spawn_object_task(
        runtime,
        TaskOptions {
            storage: Some(
                crate::storage::SqliteStorage::open(&dir.path().join("hub.db")).expect("opens"),
            ),
            ..Default::default()
        },
    );

    handle
        .call(
            "__dispatch",
            serde_json::json!({
                "class": "Hub", "name": "town", "method": "enroll",
                "args": ["ada"], "chain": [],
            }),
        )
        .await
        .expect("plain methods still run");
    let refused = handle
        .call(
            "__dispatch",
            serde_json::json!({
                "class": "Hub", "name": "town", "method": "post",
                "args": ["sport", "goal"], "chain": [],
            }),
        )
        .await;
    assert!(
        refused.is_err(),
        "a contract that never recorded the topic refuses publish"
    );
}

/// A test's vm factory: fresh vms of `SOURCE`, each wired to the
/// same router, exactly what the worker's factory does.
fn test_vm_factory(router: ObjectRouter) -> crate::connections::actor::VmFactory {
    Arc::new(move || {
        let router = router.clone();
        Box::pin(async move {
            let runtime = vm(SOURCE, false).await;
            runtime.set_app_data::<ObjectRouter>(router);
            register_test_slow(&runtime);
            Ok(runtime)
        })
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_connection_program_follows_pulls_and_sends() {
    use crate::connections::OutboundFrame;
    use crate::connections::actor::ConnectionTask;
    use crate::extensions::sockets::{PendingUpgrade, SockShared};

    let dir = tempfile::tempdir().expect("tempdir");
    let connections: Arc<crate::connections::ConnectionRegistry> = Arc::default();
    let router = town_router_with(
        dir.path().to_path_buf(),
        false,
        connections.clone(),
        None,
        None,
    );

    call(
        &router,
        "Hub",
        "town",
        "enroll",
        vec![serde_json::json!("ada")],
    )
    .await
    .expect("admits");

    // The bridge's transport half, hand-built: registered inbox in,
    // outbound frames out; no websocket anywhere in worker-core.
    let (inbox_tx, inbox_rx) = crate::connections::inbox();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<OutboundFrame>(16);
    connections.register("conn#s1", inbox_tx.clone());

    // The upgrade rides the script's own fetch handler in a request
    // vm that dies after the response: arm the request, run the
    // listener, take the parked pending, drop the vm.
    let runtime = vm(SOURCE, false).await;
    runtime.set_app_data::<ObjectRouter>(router.clone());
    let request = runtime.create_table().expect("request table");
    crate::extensions::sockets::arm_request(&runtime, &request).expect("arms");
    let listener = runtime.listener("fetch").expect("registered");
    let marker: mlua::Value = listener
        .call_async(mlua::Value::Table(request))
        .await
        .expect("the handler upgrades");
    let is_marker = marker
        .as_table()
        .and_then(|table| table.get::<bool>("__actias_upgrade").ok())
        .unwrap_or(false);
    assert!(is_marker, "the handler returned the upgrade marker");
    let pending = runtime
        .remove_app_data::<PendingUpgrade>()
        .expect("the upgrade was parked");
    assert_eq!(
        (pending.class.as_str(), pending.name.as_str()),
        ("Reader", "ada"),
        "the identity travels from the handle"
    );
    assert_eq!(
        pending.spec.name, "Echo",
        "the declared class travels whole"
    );
    drop(runtime);

    let shared = SockShared::new(
        "conn#s1".to_owned(),
        String::new(),
        "Reader".to_owned(),
        "ada".to_owned(),
        out_tx,
        router.clone(),
    );
    let task = ConnectionTask::new(
        inbox_rx,
        shared,
        pending.spec,
        pending.seed,
        test_vm_factory(router.clone()),
        Some(std::time::Duration::from_secs(60)),
        Arc::default(),
    );
    let drive = tokio::spawn(task.run());

    // The program's follow lands as an edge, then a publish flows
    // wire-ward through the pump, the inbox and the program.
    let mut audience = -1;
    for _ in 0..80 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        audience = call(&router, "Hub", "town", "audience", vec![])
            .await
            .expect("audience answers")
            .as_i64()
            .unwrap_or(-1);
        if audience == 1 {
            break;
        }
    }
    if audience != 1 {
        let early = tokio::time::timeout(std::time::Duration::from_secs(1), drive).await;
        panic!("no edge was made; the program said: {early:?}");
    }

    call(
        &router,
        "Hub",
        "town",
        "post",
        vec![serde_json::json!("sport"), serde_json::json!("goal")],
    )
    .await
    .expect("posts");
    let forwarded = tokio::time::timeout(std::time::Duration::from_secs(4), out_rx.recv())
        .await
        .expect("the event reaches the wire side")
        .expect("outbound open");
    assert_eq!(
        forwarded,
        OutboundFrame::Json(serde_json::json!({
            "kind": "event", "topic": "news", "from": "Hub/town",
        }))
    );

    // A client frame merges into the same inbox and comes back.
    inbox_tx
        .push(crate::connections::InboxItem::Frame(
            serde_json::json!({ "hello": "world" }),
        ))
        .expect("frame lands");
    let echoed = tokio::time::timeout(std::time::Duration::from_secs(4), out_rx.recv())
        .await
        .expect("the echo reaches the wire side")
        .expect("outbound open");
    assert_eq!(
        echoed,
        OutboundFrame::Json(serde_json::json!({ "kind": "frame", "echo": "world" }))
    );

    // The frame handler asked to close: the bridge is told, the
    // wire (played by this test) reports Closed back, and the actor
    // severs the edge on its way out.
    let closing = tokio::time::timeout(std::time::Duration::from_secs(4), out_rx.recv())
        .await
        .expect("the close reaches the wire side")
        .expect("outbound open");
    assert_eq!(closing, OutboundFrame::Close);
    inbox_tx
        .push(crate::connections::InboxItem::Closed)
        .expect("close lands");
    drive
        .await
        .expect("drive joins")
        .expect("the connection ran cleanly");
    let after = call(&router, "Hub", "town", "audience", vec![])
        .await
        .expect("audience answers")
        .as_i64()
        .unwrap_or(-1);
    assert_eq!(after, 0, "the actor unfollowed on the way out");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_redelivered_event_skips_by_the_followers_own_cursor() {
    let dir = tempfile::tempdir().expect("tempdir");
    let router = town_router(dir.path().to_path_buf(), false);

    let event = |seq: i64| {
        serde_json::json!({
            "seq": seq,
            "topic": "news",
            "from": { "class": "Hub", "name": "town" },
            "data": { "kind": "sport", "text": "goal" },
        })
    };

    // The same seq lands twice (at-least-once will do that); the
    // handler must run once. A later seq still lands.
    call(&router, "Reader", "ada", "__receive", vec![event(7)])
        .await
        .expect("first delivery");
    call(&router, "Reader", "ada", "__receive", vec![event(7)])
        .await
        .expect("redelivery is quietly skipped");
    call(&router, "Reader", "ada", "__receive", vec![event(8)])
        .await
        .expect("the next seq lands");

    let rows = seen_rows(&router, "ada").await;
    assert_eq!(rows.len(), 2, "seq 7 ran once, seq 8 once: {rows:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_gone_node_clears_the_stale_home_and_delivery_routes_instead() {
    let dir = tempfile::tempdir().expect("tempdir");

    // The recorded node no longer exists; the forwarder says so
    // every time it is asked.
    let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let forwarder: ReceiveForwarder = {
        let asked = asked.clone();
        Arc::new(move |_node, _identity, _batch| {
            let asked = asked.clone();
            Box::pin(async move {
                asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err(super::ForwardError::NodeGone)
            })
        })
    };

    let identity = PublisherIdentity {
        scope: "p".to_owned(),
        class: "Hub".to_owned(),
        name: "town".to_owned(),
    };
    let router = town_router_with(
        dir.path().to_path_buf(),
        false,
        Arc::default(),
        Some((
            "here".to_owned(),
            Arc::new(|_, _| Box::pin(async { Ok(Vec::new()) })),
        )),
        Some((identity, forwarder)),
    );

    call(
        &router,
        "Hub",
        "town",
        "enroll",
        vec![serde_json::json!("ada")],
    )
    .await
    .expect("admitted");
    call(
        &router,
        "Hub",
        "town",
        "__follow",
        vec![
            serde_json::json!("news"),
            serde_json::Value::Null,
            serde_json::json!({
                "class": "Reader",
                "name": "ada",
                "transport": "object",
                "node": "dead-node-id",
            }),
        ],
    )
    .await
    .expect("the gate admits the member");

    call(
        &router,
        "Hub",
        "town",
        "post",
        vec![serde_json::json!("sport"), serde_json::json!("goal")],
    )
    .await
    .expect("posted");

    // The batch hit the gone node once, then the stale home cleared:
    // no backoff, and the next pump routes the follower's identity,
    // which the local router serves, advancing the cursor.
    let mut healed = false;
    for _ in 0..120 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let file = dir.path().join("Hub_town.db");
        let mut storage = crate::storage::SqliteStorage::open(&file).expect("opens");
        let edges = list_edges(&mut storage, None).expect("edges list");
        let Some(edge) = edges.first() else { continue };
        if edge.node.is_none() && edge.attempts == 0 && edge.cursor > 0 {
            healed = true;
            break;
        }
    }
    assert!(healed, "the stale home never cleared into routed delivery");
    assert_eq!(
        asked.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the gone node should be asked exactly once"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_remote_durable_edge_batches_and_scores_like_per_edge_delivery() {
    let dir = tempfile::tempdir().expect("tempdir");

    type Heard = Arc<std::sync::Mutex<Vec<(String, String, Vec<(String, usize)>)>>>;
    let heard: Heard = Arc::default();
    let flaky_once = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let forwarder: ReceiveForwarder = {
        let heard = heard.clone();
        let flaky_once = flaky_once.clone();
        Arc::new(move |node, identity, batch| {
            let heard = heard.clone();
            let flaky_once = flaky_once.clone();
            Box::pin(async move {
                let shape = batch
                    .iter()
                    .map(|delivery| (delivery.follower_name.clone(), delivery.events.len()))
                    .collect();
                heard
                    .lock()
                    .expect("no panics")
                    .push((node, identity.name, shape));
                // First call: transport failure, so every edge in
                // the batch backs off and nothing advances.
                if flaky_once.swap(false, std::sync::atomic::Ordering::SeqCst) {
                    return Err(super::ForwardError::Transport(
                        "node unreachable".to_owned(),
                    ));
                }
                Ok(batch
                    .iter()
                    .map(|delivery| ReceiveReport {
                        follower_class: delivery.follower_class.clone(),
                        follower_name: delivery.follower_name.clone(),
                        delivered_to: delivery
                            .events
                            .iter()
                            .filter_map(|event| event["seq"].as_i64())
                            .max()
                            .unwrap_or(0),
                        failed: false,
                    })
                    .collect())
            })
        })
    };

    let identity = PublisherIdentity {
        scope: "p".to_owned(),
        class: "Hub".to_owned(),
        name: "town".to_owned(),
    };
    let router = town_router_with(
        dir.path().to_path_buf(),
        false,
        Arc::default(),
        Some((
            "here".to_owned(),
            Arc::new(|_, _| Box::pin(async { Ok(Vec::new()) })),
        )),
        Some((identity, forwarder)),
    );

    call(
        &router,
        "Hub",
        "town",
        "enroll",
        vec![serde_json::json!("ada")],
    )
    .await
    .expect("admitted");
    call(
        &router,
        "Hub",
        "town",
        "__follow",
        vec![
            serde_json::json!("news"),
            serde_json::Value::Null,
            serde_json::json!({
                "class": "Reader",
                "name": "ada",
                "transport": "object",
                "node": "elsewhere",
            }),
        ],
    )
    .await
    .expect("the gate admits the member");

    call(
        &router,
        "Hub",
        "town",
        "post",
        vec![serde_json::json!("sport"), serde_json::json!("goal")],
    )
    .await
    .expect("posted");

    // Two forwarder calls arrive: the failed one, then the retry
    // after backoff... the retry waits `DELIVERY_TIMEOUT_SECS`; assert
    // the first (failed) call and the backoff bookkeeping instead.
    let first = 'wait: {
        for _ in 0..80 {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let seen = heard.lock().expect("no panics").clone();
            if !seen.is_empty() {
                break 'wait seen;
            }
        }
        panic!("the forwarder never heard the durable batch");
    };
    let (node, publisher, shape) = &first[0];
    assert_eq!(node, "elsewhere");
    assert_eq!(publisher, "town");
    assert_eq!(shape, &vec![("ada".to_owned(), 1usize)]);

    let file = dir.path().join("Hub_town.db");
    let mut storage = crate::storage::SqliteStorage::open(&file).expect("opens");
    let edge = list_edges(&mut storage, Some("news"))
        .expect("edges")
        .into_iter()
        .find(|edge| edge.kind == "object")
        .expect("the durable edge survives a transport failure");
    assert_eq!(edge.cursor, 0, "nothing advanced on the failed send");
    assert!(edge.attempts >= 1, "the transport failure backed off");
    assert!(edge.next_at > 0, "a retry is scheduled");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_remote_connection_edge_batches_through_the_forwarder() {
    let dir = tempfile::tempdir().expect("tempdir");

    // What each "node" heard: (node, events shipped, connections).
    type Heard = Arc<std::sync::Mutex<Vec<(String, usize, Vec<String>)>>>;
    let heard: Heard = Arc::default();
    let forwarder: ConnectionForwarder = {
        let heard = heard.clone();
        Arc::new(move |node, batch: NodeInbox| {
            let heard = heard.clone();
            Box::pin(async move {
                let mut connections: Vec<String> = batch
                    .edges
                    .iter()
                    .map(|edge| edge.connection.clone())
                    .collect();
                connections.sort_unstable();
                connections.dedup();
                heard
                    .lock()
                    .expect("no panics")
                    .push((node, batch.events.len(), connections));
                // The second connection is gone wherever it went;
                // none of its edges may survive the receipt.
                Ok(vec!["conn#gone".to_owned()])
            })
        })
    };

    let router = town_router_with(
        dir.path().to_path_buf(),
        false,
        Arc::default(),
        Some(("here".to_owned(), forwarder)),
        None,
    );

    call(
        &router,
        "Hub",
        "town",
        "enroll",
        vec![serde_json::json!("ada")],
    )
    .await
    .expect("admitted");
    // Three edges on one node: two connections on "news", and a
    // second topic on the connection that is about to be gone, so
    // the prune must take both of its edges.
    for (connection, topic) in [
        ("conn#far", "news"),
        ("conn#gone", "news"),
        ("conn#gone", "noise"),
    ] {
        call(
            &router,
            "Hub",
            "town",
            "__follow",
            vec![
                serde_json::json!(topic),
                serde_json::Value::Null,
                serde_json::json!({
                    "class": "Reader",
                    "name": "ada",
                    "transport": "connection",
                    "connection": connection,
                    "node": "elsewhere",
                }),
            ],
        )
        .await
        .expect("the gate admits the member's connection");
    }

    call(
        &router,
        "Hub",
        "town",
        "post",
        vec![serde_json::json!("sport"), serde_json::json!("goal")],
    )
    .await
    .expect("posted");

    // The pump runs between mailbox items; poll until the batch
    // lands.
    let batches = 'wait: {
        for _ in 0..80 {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let seen = heard.lock().expect("no panics").clone();
            if !seen.is_empty() {
                break 'wait seen;
            }
        }
        panic!("the forwarder never heard the batch");
    };

    // One call to the one node, both connections riding it, and
    // the one posted event travels once however many edges want
    // it: the payload never multiplies by listeners.
    assert_eq!(batches.len(), 1, "one node, one send: {batches:?}");
    let (node, events, connections) = &batches[0];
    assert_eq!(node, "elsewhere");
    assert_eq!(*events, 1, "the event ships once per node");
    assert_eq!(connections, &vec!["conn#far", "conn#gone"]);

    // The receipt pruned every edge the gone connection held, the
    // second topic included, and advanced the survivor.
    let file = dir.path().join("Hub_town.db");
    let mut storage = crate::storage::SqliteStorage::open(&file).expect("opens");
    let survivors: Vec<Edge> = list_edges(&mut storage, None)
        .expect("edges list")
        .into_iter()
        .filter(|edge| edge.kind == "connection")
        .collect();
    assert_eq!(survivors.len(), 1, "both gone edges were pruned");
    assert_eq!(survivors[0].connection.as_deref(), Some("conn#far"));
    let head = head_seq(&mut storage).expect("head");
    assert_eq!(survivors[0].cursor, head, "at-most-once advanced to head");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_followers_read_answers_read_only() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("hub.db");
    {
        let mut storage = crate::storage::SqliteStorage::open(&file).expect("opens");
        // Follow first: a fresh edge starts at head (no backfill),
        // so lag only accrues for events published after it.
        let sport = serde_json::json!({ "kind": "sport" });
        crate::streams::upsert_edge(
            &mut storage,
            crate::streams::EdgeSpec {
                kind: "object",
                class: "Reader",
                name: "ada",
                connection_id: None,
                topic: "news",
                filter: None,
                node: None,
            },
        )
        .expect("edges");
        crate::streams::upsert_edge(
            &mut storage,
            crate::streams::EdgeSpec {
                kind: "connection",
                class: "Reader",
                name: "ada",
                connection_id: Some("conn#7"),
                topic: "news",
                filter: Some(&sport),
                node: None,
            },
        )
        .expect("edges");
        crate::streams::append_event(
            &mut storage,
            ("Hub", "town"),
            "news",
            &serde_json::json!({ "kind": "sport" }),
        )
        .expect("appends");
        storage.checkpoint().ok();
    }

    let mut read_only =
        crate::storage::SqliteStorage::open_read_only(&file).expect("opens read-only");
    let value = crate::streams::read_followers(&mut read_only).expect("reads");
    assert_eq!(value["head"], 1);
    let edges = value["edges"].as_array().expect("edges");
    assert_eq!(edges.len(), 2);
    assert_eq!(edges[0]["follower"], "Reader/ada");
    assert_eq!(edges[0]["kind"], "object");
    assert_eq!(edges[0]["lag"], 1, "cursor 0 against head 1");
    assert_eq!(edges[1]["kind"], "connection");
    assert_eq!(edges[1]["connection"], "conn#7");
    assert_eq!(edges[1]["lag"], serde_json::Value::Null);
    assert_eq!(edges[1]["filter"]["kind"], "sport");

    // A file that never touched streams answers empty, not error.
    let plain = dir.path().join("plain.db");
    crate::storage::SqliteStorage::open(&plain).expect("opens");
    let mut plain_read =
        crate::storage::SqliteStorage::open_read_only(&plain).expect("opens read-only");
    let empty = crate::streams::read_followers(&mut plain_read).expect("reads");
    assert_eq!(empty["edges"].as_array().map(Vec::len), Some(0));
}

/// `"public"` is a built-in policy like `"self"`: a broadcast topic
/// admits any identity with no gate code at all, because after the
/// fifteenth revision every follow is server-authored and a
/// yes-to-everyone gate is boilerplate guarding against yourself.
#[tokio::test(flavor = "multi_thread")]
async fn a_public_topic_needs_no_gate() {
    let dir = tempfile::tempdir().expect("tempdir");
    let router = town_router(dir.path().to_path_buf(), false);

    let stranger = serde_json::json!({
        "class": "Stranger", "name": "nobody", "transport": "object",
    });

    // The gate refuses a class it never heard of on a hooked topic.
    let refused = call(
        &router,
        "Hub",
        "town",
        "__follow",
        vec![
            serde_json::json!("news"),
            serde_json::json!(null),
            stranger.clone(),
        ],
    )
    .await;
    assert!(refused.is_err(), "hooked topics still gate");

    // The public topic admits the same identity, no gate consulted.
    call(
        &router,
        "Hub",
        "town",
        "__follow",
        vec![
            serde_json::json!("plaza"),
            serde_json::json!(null),
            stranger,
        ],
    )
    .await
    .expect("public topics admit anyone");
}

/// The pcall landmine, closed: guarding a cross-object call with
/// pcall is the natural style, and it works only while pcall is
/// Luau's native (yieldable) implementation. mlua's
/// catch_rust_panics(false) silently swaps in a plain-C wrapper
/// that no yield can cross, which 500'd real handlers live; this
/// pins both arms (success and a caught refusal) in a dispatched
/// method AND in the fetch vm, the exact site that broke.
#[tokio::test(flavor = "multi_thread")]
async fn pcall_hosts_cross_object_calls() {
    let dir = tempfile::tempdir().expect("tempdir");
    let router = town_router(dir.path().to_path_buf(), false);

    // Inside object dispatch.
    let guarded = call(&router, "Reader", "ada", "guarded", vec![])
        .await
        .expect("the guard method answers");
    assert_eq!(guarded["ok"], true, "{guarded}");
    assert!(guarded["value"].is_number(), "{guarded}");
    assert_eq!(guarded["bad"], false, "{guarded}");
    assert!(
        guarded["why"]
            .as_str()
            .unwrap_or_default()
            .contains("not in this class's publishes"),
        "the refusal is CAUGHT, not a wedge: {guarded}"
    );

    // Inside a fetch vm, the exact live-failure site.
    let runtime = vm(SOURCE, false).await;
    runtime.set_app_data::<ObjectRouter>(router.clone());
    let request = runtime.create_table().expect("request table");
    request.set("guard_probe", true).expect("flag");
    let listener = runtime.listener("fetch").expect("registered");
    let answer: mlua::Value = listener
        .call_async(mlua::Value::Table(request))
        .await
        .expect("the handler answers instead of 500ing");
    use mlua::LuaSerdeExt;
    let answer: serde_json::Value = runtime.from_value(answer).expect("converts");
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(answer["bad"], false, "{answer}");
    assert!(
        answer["why"]
            .as_str()
            .unwrap_or_default()
            .contains("not in this class's publishes"),
        "{answer}"
    );
}

/// Pins the Luau restriction the seventeenth revision rests on: a
/// `coroutine.yield` under the native implementation cannot cross a generic-for iterator call
/// (no mlua machinery involved at all). If a Luau upgrade ever
/// makes this pass differently, `for item in sock:each()` becomes
/// possible and the surface deserves revisiting.
#[tokio::test(flavor = "multi_thread")]
async fn a_native_yield_cannot_cross_generic_for() {
    let dir = tempfile::tempdir().expect("tempdir");
    let router = town_router(dir.path().to_path_buf(), false);
    let verdict = call(&router, "Hub", "town", "forin_pure_yield", vec![])
        .await
        .expect("probe answers");
    assert_eq!(verdict["ok1"], false, "the yield was refused: {verdict}");
    assert!(
        verdict["ask"]
            .as_str()
            .unwrap_or_default()
            .contains("yield across"),
        "refused for the pinned reason: {verdict}"
    );
}

/// The written-out one-stream socket: a declared event handler,
/// so the app owns the wire shape rather than taking the
/// forwarded envelope.
#[tokio::test(flavor = "multi_thread")]
async fn a_one_stream_program_pushes_shaped_frames() {
    use crate::connections::OutboundFrame;
    use crate::connections::actor::ConnectionTask;
    use crate::extensions::sockets::{PendingUpgrade, SockShared};

    let dir = tempfile::tempdir().expect("tempdir");
    let connections: Arc<crate::connections::ConnectionRegistry> = Arc::default();
    let router = town_router_with(
        dir.path().to_path_buf(),
        false,
        connections.clone(),
        None,
        None,
    );

    call(
        &router,
        "Hub",
        "town",
        "enroll",
        vec![serde_json::json!("ada")],
    )
    .await
    .expect("admits");

    let (inbox_tx, inbox_rx) = crate::connections::inbox();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<OutboundFrame>(16);
    connections.register("conn#f1", inbox_tx.clone());

    let runtime = vm(SOURCE, false).await;
    runtime.set_app_data::<ObjectRouter>(router.clone());
    let request = runtime.create_table().expect("request table");
    request.set("wants_forward", true).expect("flag");
    crate::extensions::sockets::arm_request(&runtime, &request).expect("arms");
    let listener = runtime.listener("fetch").expect("registered");
    let _: mlua::Value = listener
        .call_async(mlua::Value::Table(request))
        .await
        .expect("the handler upgrades");
    let pending = runtime
        .remove_app_data::<PendingUpgrade>()
        .expect("the upgrade was parked");
    drop(runtime);

    let shared = SockShared::new(
        "conn#f1".to_owned(),
        String::new(),
        pending.class.clone(),
        pending.name.clone(),
        out_tx,
        router.clone(),
    );
    let task = ConnectionTask::new(
        inbox_rx,
        shared,
        pending.spec,
        pending.seed,
        test_vm_factory(router.clone()),
        Some(std::time::Duration::from_secs(60)),
        Arc::default(),
    );
    let drive = tokio::spawn(task.run());

    let mut audience = -1;
    for _ in 0..80 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        audience = call(&router, "Hub", "town", "audience", vec![])
            .await
            .expect("audience answers")
            .as_i64()
            .unwrap_or(-1);
        if audience == 1 {
            break;
        }
    }
    if audience != 1 {
        let early = tokio::time::timeout(std::time::Duration::from_secs(1), drive).await;
        panic!("no edge was made; the program said: {early:?}");
    }

    call(
        &router,
        "Hub",
        "town",
        "post",
        vec![serde_json::json!("sport"), serde_json::json!("goal")],
    )
    .await
    .expect("posts");
    let forwarded = tokio::time::timeout(std::time::Duration::from_secs(4), out_rx.recv())
        .await
        .expect("the event reaches the wire side")
        .expect("outbound open");
    let OutboundFrame::Json(frame) = forwarded else {
        panic!("expected a frame, got {forwarded:?}");
    };
    assert_eq!(frame["topic"], "news");
    assert_eq!(
        frame["from"], "Hub/town",
        "the APP's shape, not the envelope"
    );
    assert_eq!(frame["kind"], "sport");

    // Closing the wire ends the actor: Closed drains, edges sever.
    inbox_tx
        .push(crate::connections::InboxItem::Closed)
        .expect("close lands");
    drive
        .await
        .expect("drive joins")
        .expect("the connection ran cleanly");
}

/// The bridge for the timer tests: an upgraded Beat task with a
/// tiny hibernate threshold, which the timer must override.
async fn beat_task() -> (
    crate::connections::InboxSender,
    tokio::sync::mpsc::Receiver<crate::connections::OutboundFrame>,
    Arc<crate::connections::actor::ConnectionGauges>,
    tokio::task::JoinHandle<Result<(), String>>,
    tempfile::TempDir,
) {
    use crate::connections::actor::ConnectionTask;
    use crate::extensions::sockets::{PendingUpgrade, SockShared};

    let dir = tempfile::tempdir().expect("tempdir");
    let router = town_router(dir.path().to_path_buf(), false);

    let (inbox_tx, inbox_rx) = crate::connections::inbox();
    let (out_tx, out_rx) = tokio::sync::mpsc::channel(16);

    let runtime = vm(SOURCE, false).await;
    runtime.set_app_data::<ObjectRouter>(router.clone());
    let request = runtime.create_table().expect("request table");
    request.set("wants_beat", true).expect("flag");
    crate::extensions::sockets::arm_request(&runtime, &request).expect("arms");
    let listener = runtime.listener("fetch").expect("registered");
    let _: mlua::Value = listener
        .call_async(mlua::Value::Table(request))
        .await
        .expect("the handler upgrades");
    let pending = runtime
        .remove_app_data::<PendingUpgrade>()
        .expect("the upgrade was parked");
    drop(runtime);

    let shared = SockShared::new(
        "conn#b1".to_owned(),
        String::new(),
        pending.class.clone(),
        pending.name.clone(),
        out_tx,
        router.clone(),
    );
    let gauges: Arc<crate::connections::actor::ConnectionGauges> = Arc::default();
    let task = ConnectionTask::new(
        inbox_rx,
        shared,
        pending.spec,
        pending.seed,
        test_vm_factory(router),
        Some(std::time::Duration::from_millis(150)),
        gauges.clone(),
    );
    (inbox_tx, out_rx, gauges, tokio::spawn(task.run()), dir)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_timer_ticks_coalesces_lateness_and_keeps_the_vm_warm() {
    use crate::connections::{InboxItem, OutboundFrame};
    use std::sync::atomic::Ordering;

    let (inbox_tx, mut out_rx, gauges, drive, _dir) = beat_task().await;

    let tick = |raw: Option<OutboundFrame>| -> (i64, i64) {
        let Some(OutboundFrame::Json(frame)) = raw else {
            panic!("expected a tick frame, got {raw:?}");
        };
        assert_eq!(frame["kind"], "tick");
        (
            frame["n"].as_i64().expect("n"),
            frame["missed"].as_i64().expect("missed"),
        )
    };

    // First tick fires on schedule; its handler then busy-waits
    // past two deadlines, so the second tick carries the missed
    // count instead of a burst of catch-up ticks.
    let first = tokio::time::timeout(std::time::Duration::from_secs(4), out_rx.recv())
        .await
        .expect("the first tick fires");
    assert_eq!(tick(first), (1, 0), "on time, nothing missed");
    let second = tokio::time::timeout(std::time::Duration::from_secs(6), out_rx.recv())
        .await
        .expect("the second tick fires");
    let (n, missed) = tick(second);
    assert_eq!(n, 2, "coalesced: one invocation, not a queue of them");
    assert!(
        missed >= 1,
        "the slow handler's lateness was counted: {missed}"
    );

    // The timer overrode the 150ms hibernate threshold: across
    // seconds of ticking the vm never dropped.
    assert_eq!(gauges.hibernated.load(Ordering::Relaxed), 0);
    assert_eq!(gauges.warm.load(Ordering::Relaxed), 1);
    assert_eq!(gauges.wakes.load(Ordering::Relaxed), 0);

    inbox_tx.push(InboxItem::Closed).expect("close lands");
    drive
        .await
        .expect("drive joins")
        .expect("the connection ran cleanly");
}

/// The bridge for the hibernation tests: an upgraded Sticky task
/// over a hand-built transport, plus the gauges to watch.
async fn sticky_task(
    hibernate_after: std::time::Duration,
) -> (
    crate::connections::InboxSender,
    tokio::sync::mpsc::Receiver<crate::connections::OutboundFrame>,
    Arc<crate::connections::actor::ConnectionGauges>,
    tokio::task::JoinHandle<Result<(), String>>,
    tempfile::TempDir,
) {
    use crate::connections::actor::ConnectionTask;
    use crate::extensions::sockets::{PendingUpgrade, SockShared};

    let dir = tempfile::tempdir().expect("tempdir");
    let router = town_router(dir.path().to_path_buf(), false);

    let (inbox_tx, inbox_rx) = crate::connections::inbox();
    let (out_tx, out_rx) = tokio::sync::mpsc::channel(16);

    let runtime = vm(SOURCE, false).await;
    runtime.set_app_data::<ObjectRouter>(router.clone());
    let request = runtime.create_table().expect("request table");
    request.set("wants_sticky", true).expect("flag");
    crate::extensions::sockets::arm_request(&runtime, &request).expect("arms");
    let listener = runtime.listener("fetch").expect("registered");
    let _: mlua::Value = listener
        .call_async(mlua::Value::Table(request))
        .await
        .expect("the handler upgrades");
    let pending = runtime
        .remove_app_data::<PendingUpgrade>()
        .expect("the upgrade was parked");
    drop(runtime);

    let shared = SockShared::new(
        "conn#h1".to_owned(),
        String::new(),
        pending.class.clone(),
        pending.name.clone(),
        out_tx,
        router.clone(),
    );
    let gauges: Arc<crate::connections::actor::ConnectionGauges> = Arc::default();
    let task = ConnectionTask::new(
        inbox_rx,
        shared,
        pending.spec,
        pending.seed,
        test_vm_factory(router),
        Some(hibernate_after),
        gauges.clone(),
    );
    (inbox_tx, out_rx, gauges, tokio::spawn(task.run()), dir)
}

/// Polls a gauge pair until it matches or the deadline passes.
async fn wait_gauges(
    gauges: &crate::connections::actor::ConnectionGauges,
    warm: i64,
    hibernated: i64,
) {
    use std::sync::atomic::Ordering;
    for _ in 0..80 {
        if gauges.warm.load(Ordering::Relaxed) == warm
            && gauges.hibernated.load(Ordering::Relaxed) == hibernated
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!(
        "gauges never reached warm={warm} hibernated={hibernated}; at warm={} hibernated={}",
        gauges.warm.load(Ordering::Relaxed),
        gauges.hibernated.load(Ordering::Relaxed)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_idle_connection_drops_its_vm_and_a_delivery_revives_it() {
    use crate::connections::{InboxItem, OutboundFrame};
    use std::sync::atomic::Ordering;

    let (inbox_tx, mut out_rx, gauges, drive, _dir) =
        sticky_task(std::time::Duration::from_millis(150)).await;

    inbox_tx
        .push(InboxItem::Frame(serde_json::json!({})))
        .expect("frame lands");
    let first = tokio::time::timeout(std::time::Duration::from_secs(4), out_rx.recv())
        .await
        .expect("the count answers")
        .expect("outbound open");
    assert_eq!(
        first,
        OutboundFrame::Json(serde_json::json!({ "kind": "count", "n": 1 }))
    );
    wait_gauges(&gauges, 1, 0).await;

    // Silence past the threshold: the vm falls, the task stays.
    wait_gauges(&gauges, 0, 1).await;

    // The next frame is the wake; the blob survived the vm.
    inbox_tx
        .push(InboxItem::Frame(serde_json::json!({})))
        .expect("frame lands");
    let second = tokio::time::timeout(std::time::Duration::from_secs(4), out_rx.recv())
        .await
        .expect("the count answers")
        .expect("outbound open");
    assert_eq!(
        second,
        OutboundFrame::Json(serde_json::json!({ "kind": "count", "n": 2 }))
    );
    assert_eq!(gauges.wakes.load(Ordering::Relaxed), 1, "one wake, counted");
    wait_gauges(&gauges, 1, 0).await;

    inbox_tx.push(InboxItem::Closed).expect("close lands");
    drive
        .await
        .expect("drive joins")
        .expect("the connection ran cleanly");
    wait_gauges(&gauges, 0, 0).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_close_without_a_close_handler_never_wakes_a_hibernated_vm() {
    use crate::connections::InboxItem;
    use std::sync::atomic::Ordering;

    let (inbox_tx, mut out_rx, gauges, drive, _dir) =
        sticky_task(std::time::Duration::from_millis(150)).await;

    inbox_tx
        .push(InboxItem::Frame(serde_json::json!({})))
        .expect("frame lands");
    let _ = tokio::time::timeout(std::time::Duration::from_secs(4), out_rx.recv())
        .await
        .expect("the count answers");
    wait_gauges(&gauges, 0, 1).await;

    // Sticky declares no close handler, so ending the wire must
    // not pay a vm build to discover there is nothing to run.
    inbox_tx.push(InboxItem::Closed).expect("close lands");
    drive
        .await
        .expect("drive joins")
        .expect("the connection ran cleanly");
    assert_eq!(
        gauges.wakes.load(Ordering::Relaxed),
        0,
        "no wake for an undeclared handler"
    );
    wait_gauges(&gauges, 0, 0).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_forward_policy_sends_without_a_wake() {
    use crate::connections::{InboxItem, OutboundFrame};
    use std::sync::atomic::Ordering;

    let (inbox_tx, mut out_rx, gauges, drive, _dir) =
        sticky_task(std::time::Duration::from_millis(150)).await;

    // Warm the vm with a frame, then let it hibernate.
    inbox_tx
        .push(InboxItem::Frame(serde_json::json!({})))
        .expect("frame lands");
    let _ = tokio::time::timeout(std::time::Duration::from_secs(4), out_rx.recv())
        .await
        .expect("the count answers");
    wait_gauges(&gauges, 0, 1).await;

    // Sticky declares event = "forward", so a delivered event
    // reaches the wire in the platform envelope with the vm still
    // down: this is what makes hibernation compatible with being
    // a follower.
    inbox_tx
        .push(InboxItem::Event {
            topic: "news".to_owned(),
            from_class: "Hub".to_owned(),
            from_name: "town".to_owned(),
            data: serde_json::json!({ "kind": "sport" }),
        })
        .expect("event lands");
    let frame = tokio::time::timeout(std::time::Duration::from_secs(4), out_rx.recv())
        .await
        .expect("the forward answers")
        .expect("outbound open");
    assert_eq!(
        frame,
        OutboundFrame::Json(serde_json::json!({
            "type": "event",
            "topic": "news",
            "from": { "id": "Hub/town", "class": "Hub", "name": "town" },
            "data": { "kind": "sport" },
        }))
    );
    assert_eq!(
        gauges.wakes.load(Ordering::Relaxed),
        0,
        "no wake to forward"
    );
    wait_gauges(&gauges, 0, 1).await;

    inbox_tx.push(InboxItem::Closed).expect("close lands");
    drive
        .await
        .expect("drive joins")
        .expect("the connection ran cleanly");
    wait_gauges(&gauges, 0, 0).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn connection_edges_deliver_at_most_once_and_prune() {
    let dir = tempfile::tempdir().expect("tempdir");
    let connections: Arc<crate::connections::ConnectionRegistry> = Arc::default();
    let router = town_router_with(
        dir.path().to_path_buf(),
        false,
        connections.clone(),
        None,
        None,
    );

    call(
        &router,
        "Hub",
        "town",
        "enroll",
        vec![serde_json::json!("ada")],
    )
    .await
    .expect("admits");

    let (tx, mut rx) = crate::connections::inbox();
    connections.register("conn#1", tx);

    // The device follows as Reader/ada via conn#1: exactly the
    // __follow a sock:follow will send once the upgrade seam
    // exists. The gate sees the same one identity shape.
    call(
        &router,
        "Hub",
        "town",
        "__follow",
        vec![
            serde_json::json!("news"),
            serde_json::json!(null),
            serde_json::json!({
                "class": "Reader", "name": "ada",
                "transport": "connection", "connection": "conn#1",
            }),
        ],
    )
    .await
    .expect("the gate admits the member's device");

    call(
        &router,
        "Hub",
        "town",
        "post",
        vec![serde_json::json!("sport"), serde_json::json!("goal")],
    )
    .await
    .expect("posts");

    let delivered = tokio::time::timeout(std::time::Duration::from_secs(4), rx.next())
        .await
        .expect("delivery reaches the inbox before the timeout")
        .expect("inbox open");
    match delivered {
        crate::connections::InboxItem::Event {
            topic,
            from_class,
            from_name,
            data,
        } => {
            assert_eq!(topic, "news");
            assert_eq!((from_class.as_str(), from_name.as_str()), ("Hub", "town"));
            assert_eq!(data["kind"], "sport");
        }
        other => panic!("expected an event, got {other:?}"),
    }

    // The wire goes away; the next delivery prunes the edge
    // (deliver-or-prune, no retry, no backlog for dead tabs).
    connections.unregister("conn#1");
    call(
        &router,
        "Hub",
        "town",
        "post",
        vec![serde_json::json!("sport"), serde_json::json!("again")],
    )
    .await
    .expect("posts");

    let mut audience = -1;
    for _ in 0..80 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        audience = call(&router, "Hub", "town", "audience", vec![])
            .await
            .expect("audience answers")
            .as_i64()
            .unwrap_or(-1);
        if audience == 0 {
            break;
        }
    }
    assert_eq!(audience, 0, "the dead connection's edge is pruned");
}

#[tokio::test(flavor = "multi_thread")]
async fn hooks_receive_is_refused_at_declaration() {
    let source = r#"
        object "Relic" {
            hooks = {
                receive = function(state, event) end,
            },
        }
        on "fetch" (function() return { body = "ok" } end)
    "#;
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
    let refused = ActiasRuntime::new(
        prepared,
        KvServiceClient::new(crate::plain_grpc(channel)),
        crate::egress::EgressClient::new(crate::egress::EgressPolicy::new([], false))
            .expect("egress builds"),
        None,
        None,
        None,
    )
    .await;
    let error = refused.err().expect("the funnel spelling must refuse");
    assert!(
        error.to_string().contains("hooks.receive"),
        "the refusal names the dead spelling: {error}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_undeclared_stream_discards_instead_of_retrying() {
    let dir = tempfile::tempdir().expect("tempdir");
    let router = town_router(dir.path().to_path_buf(), false);

    call(
        &router,
        "Hub",
        "town",
        "enroll",
        vec![serde_json::json!("ada")],
    )
    .await
    .expect("admits");
    call(
        &router,
        "Reader",
        "ada",
        "eavesdrop",
        vec![serde_json::json!("town")],
    )
    .await
    .expect("the gate admits members; the runtime does not pre-check receives");
    call(
        &router,
        "Reader",
        "ada",
        "join",
        vec![serde_json::json!("town")],
    )
    .await
    .expect("joins");

    call(&router, "Hub", "town", "blast", vec![])
        .await
        .expect("blasts");
    call(
        &router,
        "Hub",
        "town",
        "post",
        vec![serde_json::json!("sport"), serde_json::json!("goal")],
    )
    .await
    .expect("posts");

    // The declared stream arrives; the undeclared one was consumed
    // by delivery (Ok, cursor advanced) and simply never lands.
    let rows = wait_for(&router, "ada", |rows| !rows.is_empty()).await;
    assert_eq!(
        rows.len(),
        1,
        "only Hub:news has a receives entry: {rows:?}"
    );
    assert_eq!(rows[0]["topic"], "news");
}

#[tokio::test(flavor = "multi_thread")]
async fn follows_gate_and_deliveries_flow_with_filters() {
    let dir = tempfile::tempdir().expect("tempdir");
    let router = town_router(dir.path().to_path_buf(), false);

    // Not a member yet: the gate refuses.
    let refused = call(
        &router,
        "Reader",
        "ada",
        "join",
        vec![serde_json::json!("town")],
    )
    .await;
    assert!(refused.is_err(), "gate must refuse a non-member");

    call(
        &router,
        "Hub",
        "town",
        "enroll",
        vec![serde_json::json!("ada")],
    )
    .await
    .expect("admits");
    call(
        &router,
        "Hub",
        "town",
        "enroll",
        vec![serde_json::json!("bob")],
    )
    .await
    .expect("admits");

    call(
        &router,
        "Reader",
        "ada",
        "join",
        vec![serde_json::json!("town")],
    )
    .await
    .expect("member follows");
    // bob only wants sport.
    call(
        &router,
        "Reader",
        "bob",
        "join",
        vec![serde_json::json!("town"), serde_json::json!("sport")],
    )
    .await
    .expect("filtered follow");

    assert_eq!(
        call(&router, "Hub", "town", "audience", vec![])
            .await
            .expect("audience"),
        serde_json::json!(2)
    );

    call(
        &router,
        "Hub",
        "town",
        "post",
        vec![serde_json::json!("sport"), serde_json::json!("goal")],
    )
    .await
    .expect("posts");
    call(
        &router,
        "Hub",
        "town",
        "post",
        vec![serde_json::json!("politics"), serde_json::json!("vote")],
    )
    .await
    .expect("posts");

    let ada = wait_for(&router, "ada", |rows| rows.len() >= 2).await;
    assert_eq!(ada.len(), 2, "unfiltered reader sees both: {ada:?}");
    assert_eq!(ada[0]["from_id"], "Hub/town");
    let bob = wait_for(&router, "bob", |rows| !rows.is_empty()).await;
    assert_eq!(bob.len(), 1, "filtered reader sees sport only: {bob:?}");
    assert_eq!(bob[0]["kind"], "sport");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_receive_backs_off_and_redelivers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let router = town_router(dir.path().to_path_buf(), true);

    call(
        &router,
        "Hub",
        "town",
        "enroll",
        vec![serde_json::json!("flaky")],
    )
    .await
    .expect("admits");
    call(
        &router,
        "Reader",
        "flaky",
        "join",
        vec![serde_json::json!("town")],
    )
    .await
    .expect("follows");
    call(
        &router,
        "Hub",
        "town",
        "post",
        vec![serde_json::json!("sport"), serde_json::json!("goal")],
    )
    .await
    .expect("posts");

    // First delivery trips, the edge backs off (500ms), the retry
    // lands; at-least-once made visible.
    let rows = wait_for(&router, "flaky", |rows| !rows.is_empty()).await;
    assert_eq!(rows.len(), 1, "redelivered after backoff: {rows:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn kick_drops_edges_and_reserved_names_refuse() {
    let dir = tempfile::tempdir().expect("tempdir");
    let router = town_router(dir.path().to_path_buf(), false);

    call(
        &router,
        "Hub",
        "town",
        "enroll",
        vec![serde_json::json!("ada")],
    )
    .await
    .expect("admits");
    call(
        &router,
        "Reader",
        "ada",
        "join",
        vec![serde_json::json!("town")],
    )
    .await
    .expect("follows");
    call(
        &router,
        "Hub",
        "town",
        "kick",
        vec![serde_json::json!("ada")],
    )
    .await
    .expect("kicks");
    call(
        &router,
        "Hub",
        "town",
        "post",
        vec![serde_json::json!("sport"), serde_json::json!("goal")],
    )
    .await
    .expect("posts");
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    assert!(
        seen_rows(&router, "ada").await.is_empty(),
        "a kicked identity's edges are gone"
    );

    // "self" policy: a reader may not follow the private topic.
    let refused = call(
        &router,
        "Reader",
        "ada",
        "spy",
        vec![serde_json::json!("town")],
    )
    .await;
    assert!(refused.is_err(), "self topic refuses others");

    // Undeclared topics refuse at publish.
    let leak = call(&router, "Hub", "town", "leak", vec![]).await;
    assert!(leak.is_err(), "publish outside publishes refuses");

    // Hooks are the platform's half: the public spelling refuses.
    let hook = call(&router, "Reader", "ada", "receive", vec![]).await;
    assert!(hook.is_err(), "reserved names refuse via dispatch");
}
