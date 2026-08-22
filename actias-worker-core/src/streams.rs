//! Streams: publisher-approved edges between objects, with the platform
//! owning delivery (docs/SURFACE_REV.md).
//!
//! A follow writes ONE ROW in the publisher's own SQLite; a publish
//! appends to the publisher's event log in the calling transaction; the
//! delivery pump walks edge rows after commit, copying matching events to
//! each follower's `receive` hook with a per-edge cursor, retry backoff,
//! and bounded patience. Everything rides the object's file: edges and
//! events ship with snapshots and survive takeover like any other rows.

use crate::storage::SqliteStorage;

/// Delivery batch per edge per pump pass; the pump re-arms for the rest.
const DELIVERY_BATCH: i64 = 16;
/// Base backoff after a failed delivery, doubling per attempt.
const BACKOFF_BASE_MS: i64 = 500;
/// Backoff ceiling.
const BACKOFF_CAP_MS: i64 = 60_000;
/// Attempts after which an edge is dropped: bounded patience, the
/// queue's dead-letter discipline applied per edge.
const MAX_ATTEMPTS: i64 = 8;
/// One delivery may not hang the pump; a timed-out edge retries later,
/// which also breaks accidental call cycles between publisher and
/// follower.
pub const DELIVERY_TIMEOUT_SECS: u64 = 10;

const CREATE_EVENTS: &str = "CREATE TABLE IF NOT EXISTS __actias_stream_events (
        seq INTEGER PRIMARY KEY AUTOINCREMENT,
        at INTEGER NOT NULL,
        from_class TEXT NOT NULL,
        from_name TEXT NOT NULL,
        topic TEXT NOT NULL,
        data TEXT NOT NULL
    )";

const CREATE_FOLLOWERS: &str = "CREATE TABLE IF NOT EXISTS __actias_followers (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        kind TEXT NOT NULL,
        class TEXT NOT NULL,
        name TEXT NOT NULL,
        connection TEXT,
        topic TEXT NOT NULL,
        filter TEXT,
        cursor INTEGER NOT NULL DEFAULT 0,
        attempts INTEGER NOT NULL DEFAULT 0,
        next_at INTEGER NOT NULL DEFAULT 0
    )";

/// Creates the stream tables when absent; idempotent, called from every
/// verb that touches them.
pub fn ensure_tables(storage: &mut SqliteStorage) -> Result<(), String> {
    let connection = storage.platform();
    connection
        .execute(CREATE_EVENTS, [])
        .map_err(|e| e.to_string())?;
    connection
        .execute(CREATE_FOLLOWERS, [])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Appends one published event in the calling transaction and returns
/// its sequence number.
pub fn append_event(
    storage: &mut SqliteStorage,
    from: (&str, &str),
    topic: &str,
    data: &serde_json::Value,
) -> Result<i64, String> {
    ensure_tables(storage)?;
    let connection = storage.platform();
    connection
        .execute(
            "INSERT INTO __actias_stream_events (at, from_class, from_name, topic, data)              VALUES (?, ?, ?, ?, ?)",
            rusqlite::params![
                crate::extensions::objects::unix_now_ms(),
                from.0,
                from.1,
                topic,
                data.to_string()
            ],
        )
        .map_err(|e| e.to_string())?;
    Ok(connection.last_insert_rowid())
}

/// One edge row, as the pump and the `followers` verb read it.
#[derive(Clone, Debug)]
pub struct Edge {
    pub id: i64,
    pub kind: String,
    pub class: String,
    pub name: String,
    pub topic: String,
    pub filter: Option<serde_json::Value>,
    pub cursor: i64,
    pub attempts: i64,
    pub next_at: i64,
}

/// Records (or re-records) an accepted follow: one edge per
/// (kind, class, name, topic); re-following replaces the filter and
/// resets patience, keeping the cursor so no history replays.
pub fn upsert_edge(
    storage: &mut SqliteStorage,
    kind: &str,
    class: &str,
    name: &str,
    topic: &str,
    filter: Option<&serde_json::Value>,
) -> Result<(), String> {
    ensure_tables(storage)?;
    let filter_text = filter.map(|value| value.to_string());
    let connection = storage.platform();
    let updated = connection
        .execute(
            "UPDATE __actias_followers SET filter = ?, attempts = 0, next_at = 0 \
             WHERE kind = ? AND class = ? AND name = ? AND topic = ?",
            rusqlite::params![filter_text, kind, class, name, topic],
        )
        .map_err(|e| e.to_string())?;
    if updated == 0 {
        // A fresh edge starts at NOW: no backfill (docs OPEN 2); history
        // is the publisher's state and an ordinary method.
        let head: i64 = connection
            .query_row(
                "SELECT COALESCE(MAX(seq), 0) FROM __actias_stream_events",
                [],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        connection
            .execute(
                "INSERT INTO __actias_followers \
                     (kind, class, name, connection, topic, filter, cursor) \
                 VALUES (?, ?, ?, NULL, ?, ?, ?)",
                rusqlite::params![kind, class, name, topic, filter_text, head],
            )
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Removes one identity's edge on one topic; unilateral, no gate.
pub fn delete_edge(
    storage: &mut SqliteStorage,
    class: &str,
    name: &str,
    topic: &str,
) -> Result<(), String> {
    ensure_tables(storage)?;
    storage
        .platform()
        .execute(
            "DELETE FROM __actias_followers WHERE class = ? AND name = ? AND topic = ?",
            rusqlite::params![class, name, topic],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Removes every edge an identity holds here, any topic, any kind.
pub fn drop_identity(
    storage: &mut SqliteStorage,
    class: &str,
    name: &str,
) -> Result<usize, String> {
    ensure_tables(storage)?;
    storage
        .platform()
        .execute(
            "DELETE FROM __actias_followers WHERE class = ? AND name = ?",
            rusqlite::params![class, name],
        )
        .map_err(|e| e.to_string())
}

/// Every edge, optionally narrowed to one topic; the `followers` verb.
pub fn list_edges(storage: &mut SqliteStorage, topic: Option<&str>) -> Result<Vec<Edge>, String> {
    ensure_tables(storage)?;
    let connection = storage.platform();
    let mut statement = connection
        .prepare(
            "SELECT id, kind, class, name, topic, filter, cursor, attempts, next_at \
             FROM __actias_followers WHERE (?1 IS NULL OR topic = ?1) ORDER BY id",
        )
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map(rusqlite::params![topic], |row| {
            Ok(Edge {
                id: row.get(0)?,
                kind: row.get(1)?,
                class: row.get(2)?,
                name: row.get(3)?,
                topic: row.get(4)?,
                filter: row
                    .get::<_, Option<String>>(5)?
                    .and_then(|text| serde_json::from_str(&text).ok()),
                cursor: row.get(6)?,
                attempts: row.get(7)?,
                next_at: row.get(8)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut edges = Vec::new();
    for row in rows {
        edges.push(row.map_err(|e| e.to_string())?);
    }
    Ok(edges)
}

/// One event as the pump reads it back; `from` rides the row so a
/// rebooted publisher stamps history correctly before any new publish.
pub struct StoredEvent {
    pub seq: i64,
    pub from_class: String,
    pub from_name: String,
    pub topic: String,
    pub data: serde_json::Value,
}

/// Events past `cursor` on `topic`, oldest first, bounded.
pub fn events_after(
    storage: &mut SqliteStorage,
    topic: &str,
    cursor: i64,
) -> Result<Vec<StoredEvent>, String> {
    let connection = storage.platform();
    let mut statement = connection
        .prepare(
            "SELECT seq, from_class, from_name, topic, data FROM __actias_stream_events              WHERE topic = ? AND seq > ? ORDER BY seq LIMIT ?",
        )
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map(rusqlite::params![topic, cursor, DELIVERY_BATCH], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut events = Vec::new();
    for row in rows {
        let (seq, from_class, from_name, topic, data) = row.map_err(|e| e.to_string())?;
        events.push(StoredEvent {
            seq,
            from_class,
            from_name,
            topic,
            data: serde_json::from_str(&data).unwrap_or(serde_json::Value::Null),
        });
    }
    Ok(events)
}

/// The newest event sequence; a no-matching-events edge fast-forwards
/// its cursor here.
pub fn head_seq(storage: &mut SqliteStorage) -> Result<i64, String> {
    storage
        .platform()
        .query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM __actias_stream_events",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())
}

/// Whether an event's data passes an edge's filter: equality on every
/// filter key against the same-named top-level data field.
pub fn filter_matches(filter: Option<&serde_json::Value>, data: &serde_json::Value) -> bool {
    match filter.and_then(|value| value.as_object()) {
        None => true,
        Some(wanted) => wanted
            .iter()
            .all(|(key, value)| data.get(key) == Some(value)),
    }
}

/// Cursor advance after a delivered batch.
pub fn advance_cursor(storage: &mut SqliteStorage, edge_id: i64, to: i64) -> Result<(), String> {
    storage
        .platform()
        .execute(
            "UPDATE __actias_followers SET cursor = ?, attempts = 0, next_at = 0 WHERE id = ?",
            rusqlite::params![to, edge_id],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Failure bookkeeping: backoff read from the row (a partial batch may
/// have reset it), and past patience the edge is dropped. Returns
/// whether the edge survived.
pub fn record_failure(storage: &mut SqliteStorage, edge_id: i64) -> Result<bool, String> {
    let connection = storage.platform();
    let stored: i64 = connection
        .query_row(
            "SELECT attempts FROM __actias_followers WHERE id = ?",
            rusqlite::params![edge_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|e| e.to_string())?;
    let attempts = stored + 1;
    if attempts >= MAX_ATTEMPTS {
        connection
            .execute(
                "DELETE FROM __actias_followers WHERE id = ?",
                rusqlite::params![edge_id],
            )
            .map_err(|e| e.to_string())?;
        return Ok(false);
    }
    let backoff = (BACKOFF_BASE_MS << (attempts - 1).min(20)).min(BACKOFF_CAP_MS);
    connection
        .execute(
            "UPDATE __actias_followers SET attempts = ?, next_at = ? WHERE id = ?",
            rusqlite::params![
                attempts,
                crate::extensions::objects::unix_now_ms() + backoff,
                edge_id
            ],
        )
        .map_err(|e| e.to_string())?;
    Ok(true)
}

/// The earliest moment any edge still has work: undelivered events ready
/// now, or a backoff expiring later. None when fully drained.
pub fn next_delivery_due(storage: &mut SqliteStorage) -> Result<Option<i64>, String> {
    let head = head_seq(storage)?;
    let now = crate::extensions::objects::unix_now_ms();
    let mut due: Option<i64> = None;
    for edge in list_edges(storage, None)? {
        if edge.kind != "object" || edge.cursor >= head {
            continue;
        }
        let at = edge.next_at.max(now);
        due = Some(due.map_or(at, |current| current.min(at)));
    }
    Ok(due)
}

/// One delivery pass over every due object edge: matching events copied
/// to each follower's `receive`, cursors advanced, failures backed off,
/// then the timer re-armed for whatever remains. Runs in the
/// publisher's own task between mailbox items.
pub async fn pump(
    runtime: &crate::runtime::ActiasRuntime,
    home: &std::sync::Arc<crate::objects::ObjectHome>,
) {
    use crate::extensions::objects::{ObjectRouter, ObjectTarget};

    if !home.has_storage() {
        return;
    }
    let Some(router) = runtime
        .app_data_ref::<ObjectRouter>()
        .map(|router| router.clone())
    else {
        actias_common::tracing::warn!("stream delivery has no router; events wait");
        return;
    };

    let now = crate::extensions::objects::unix_now_ms();
    let snapshot = home.with_storage(|storage| {
        let head = head_seq(storage)?;
        let edges = list_edges(storage, None)?;
        Ok((head, edges))
    });
    let (head, edges) = match snapshot {
        Ok(pair) => pair,
        Err(error) => {
            actias_common::tracing::warn!(%error, "stream pump could not read edges");
            return;
        }
    };

    for edge in edges {
        if edge.kind != "object" || edge.cursor >= head || edge.next_at > now {
            continue;
        }
        let events =
            match home.with_storage(|storage| events_after(storage, &edge.topic, edge.cursor)) {
                Ok(events) => events,
                Err(error) => {
                    actias_common::tracing::warn!(%error, "stream pump could not read events");
                    continue;
                }
            };
        if events.is_empty() {
            // Other topics advanced the log; nothing here for this edge.
            let _ = home.with_storage(|storage| advance_cursor(storage, edge.id, head));
            continue;
        }

        let mut advanced = edge.cursor;
        let mut failed = false;
        for event in events {
            if !filter_matches(edge.filter.as_ref(), &event.data) {
                advanced = event.seq;
                continue;
            }
            let payload = serde_json::json!({
                "topic": event.topic,
                "from": { "class": event.from_class, "name": event.from_name },
                "data": event.data,
            });
            let delivery = router(ObjectTarget {
                class: edge.class.clone(),
                name: edge.name.clone(),
                method: "__receive".to_owned(),
                arguments: vec![payload],
                // A delivery is a fresh causal root, never a nested call:
                // an empty chain keeps publisher/follower ping-pong legal,
                // and the timeout breaks accidental synchronous cycles.
                chain: Vec::new(),
                caller: None,
            });
            match tokio::time::timeout(
                std::time::Duration::from_secs(DELIVERY_TIMEOUT_SECS),
                delivery,
            )
            .await
            {
                Ok(Ok(_)) => advanced = event.seq,
                Ok(Err(error)) => {
                    actias_common::tracing::debug!(%error, "stream delivery refused");
                    failed = true;
                    break;
                }
                Err(_) => {
                    actias_common::tracing::debug!("stream delivery timed out");
                    failed = true;
                    break;
                }
            }
        }

        if advanced > edge.cursor {
            let _ = home.with_storage(|storage| advance_cursor(storage, edge.id, advanced));
        }
        if failed {
            let _ = home.with_storage(|storage| record_failure(storage, edge.id));
        }
    }

    match home.with_storage(next_delivery_due) {
        Ok(due) => home.set_delivery_due(due),
        Err(error) => {
            actias_common::tracing::warn!(%error, "stream pump could not re-arm");
        }
    }
}

#[cfg(test)]
mod tests {
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
            publishes = { "news", private = "self" },
            hooks = {
                init = function(state)
                    state.sql:exec("CREATE TABLE members (user TEXT PRIMARY KEY)")
                end,
                follow = function(state, topic, follower)
                    return follower:is(Reader) and state.sql:query_one(
                        "SELECT 1 FROM members WHERE user = ?", { follower.name }) ~= nil
                end,
            },
            admit = function(state, user)
                state.sql:exec("INSERT OR IGNORE INTO members VALUES (?)", { user })
            end,
            post = function(state, kind, text)
                state:publish("news", { kind = kind, text = text })
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
        }

        Reader = object "Reader" {
            hooks = {
                init = function(state)
                    state.sql:exec(
                        "CREATE TABLE seen (topic TEXT, kind TEXT, from_id TEXT, text TEXT)")
                end,
                receive = function(state, event)
                    if flaky_mode and state.name == "flaky" and not state.tripped then
                        state.tripped = true
                        error("transient outage")
                    end
                    state.sql:exec("INSERT INTO seen VALUES (?, ?, ?, ?)",
                        { event.topic, event.data.kind, event.from.id, event.data.text })
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
            leave = function(state, hub)
                state:unfollow(Hub(hub), "news")
            end,
            seen = function(state)
                return state.sql:query("SELECT topic, kind, from_id, text FROM seen ORDER BY rowid")
            end,
        }

        on "fetch" (function() return { body = "ok" } end)
    "#;

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
            KvServiceClient::new(channel),
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
        type Registry = Arc<tokio::sync::Mutex<std::collections::HashMap<String, ObjectHandle>>>;
        let registry: Registry = Arc::default();
        let cell: Arc<std::sync::OnceLock<ObjectRouter>> = Arc::new(std::sync::OnceLock::new());

        let router_cell = cell.clone();
        let router: ObjectRouter = Arc::new(move |target: ObjectTarget| {
            let registry = registry.clone();
            let cell = router_cell.clone();
            let dir = dir.clone();
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
            "admit",
            vec![serde_json::json!("ada")],
        )
        .await
        .expect("admits");
        call(
            &router,
            "Hub",
            "town",
            "admit",
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
            "admit",
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
            "admit",
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
}
