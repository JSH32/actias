//! Streams: publisher-approved edges between objects, with the platform
//! owning delivery.
//!
//! A follow writes ONE ROW in the publisher's own SQLite; a publish
//! appends to the publisher's event log in the calling transaction; the
//! delivery pump walks edge rows after commit, copying matching events
//! to each follower's `receives` handler with a per-edge cursor, retry
//! backoff, and bounded patience. Everything rides the object's file:
//! edges and events ship with snapshots and survive takeover like any
//! other rows.

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
    /// Connection edges only: the node-local connection this edge
    /// delivers to; the identity above is who that connection speaks AS.
    pub connection: Option<String>,
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
    connection_id: Option<&str>,
    topic: &str,
    filter: Option<&serde_json::Value>,
) -> Result<(), String> {
    ensure_tables(storage)?;
    let filter_text = filter.map(|value| value.to_string());
    let connection = storage.platform();
    // One identity may hold the same topic once per endpoint: the
    // durable edge and each connected device are separate rows (the
    // doc's two-devices table), so connection is part of the match.
    let updated = connection
        .execute(
            "UPDATE __actias_followers SET filter = ?, attempts = 0, next_at = 0 \
             WHERE kind = ? AND class = ? AND name = ? AND topic = ? AND connection IS ?",
            rusqlite::params![filter_text, kind, class, name, topic, connection_id],
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
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![kind, class, name, connection_id, topic, filter_text, head],
            )
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Deletes one edge by id: the pump's deliver-or-prune half for
/// connection edges, and dead-edge cleanup generally.
pub fn prune_edge(storage: &mut SqliteStorage, edge_id: i64) -> Result<(), String> {
    storage
        .platform()
        .execute(
            "DELETE FROM __actias_followers WHERE id = ?",
            rusqlite::params![edge_id],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Removes one identity's edge on one topic at ONE endpoint; unilateral,
/// no gate. The durable edge (NULL connection) and each device row are
/// separate endpoints, so an account unfollowing never severs its
/// devices, and a device unfollowing never severs the account.
pub fn delete_edge(
    storage: &mut SqliteStorage,
    class: &str,
    name: &str,
    connection_id: Option<&str>,
    topic: &str,
) -> Result<(), String> {
    ensure_tables(storage)?;
    storage
        .platform()
        .execute(
            "DELETE FROM __actias_followers \
             WHERE class = ? AND name = ? AND topic = ? AND connection IS ?",
            rusqlite::params![class, name, topic, connection_id],
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

/// The console's followers read: edge rows plus the event-log head,
/// from a file opened READ-ONLY, so nothing here may create tables;
/// an object that never touched streams reads as empty, not an error.
/// Object-kind edges carry their lag (head minus cursor); connection
/// edges have no cursor promise and report none.
pub fn read_followers(storage: &mut SqliteStorage) -> Result<serde_json::Value, String> {
    let connection = storage.platform();
    let present: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master \
             WHERE name IN ('__actias_followers', '__actias_stream_events')",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if present < 2 {
        return Ok(serde_json::json!({ "head": 0, "edges": [] }));
    }
    let head: i64 = connection
        .query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM __actias_stream_events",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    let mut statement = connection
        .prepare(
            "SELECT kind, class, name, connection, topic, filter, cursor, attempts, next_at \
             FROM __actias_followers ORDER BY topic, id",
        )
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([], |row| {
            let kind: String = row.get(0)?;
            let class: String = row.get(1)?;
            let name: String = row.get(2)?;
            let connection_id: Option<String> = row.get(3)?;
            let topic: String = row.get(4)?;
            let filter: Option<String> = row.get(5)?;
            let cursor: i64 = row.get(6)?;
            let attempts: i64 = row.get(7)?;
            let next_at: i64 = row.get(8)?;
            Ok(serde_json::json!({
                "kind": kind,
                "follower": format!("{class}/{name}"),
                "connection": connection_id,
                "topic": topic,
                "filter": filter
                    .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok()),
                "cursor": cursor,
                "lag": if kind == "object" { Some((head - cursor).max(0)) } else { None },
                "attempts": attempts,
                "next_at": next_at,
            }))
        })
        .map_err(|e| e.to_string())?;
    let mut edges = Vec::new();
    for row in rows {
        edges.push(row.map_err(|e| e.to_string())?);
    }
    Ok(serde_json::json!({ "head": head, "edges": edges }))
}

/// Every edge, optionally narrowed to one topic; the `followers` verb.
pub fn list_edges(storage: &mut SqliteStorage, topic: Option<&str>) -> Result<Vec<Edge>, String> {
    ensure_tables(storage)?;
    let connection = storage.platform();
    let mut statement = connection
        .prepare(
            "SELECT id, kind, class, name, connection, topic, filter, cursor, attempts, next_at \
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
                connection: row.get(4)?,
                topic: row.get(5)?,
                filter: row
                    .get::<_, Option<String>>(6)?
                    .and_then(|text| serde_json::from_str(&text).ok()),
                cursor: row.get(7)?,
                attempts: row.get(8)?,
                next_at: row.get(9)?,
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
        if edge.cursor >= head {
            continue;
        }
        // Connection edges never back off: pending means due now.
        let at = if edge.kind == "object" {
            edge.next_at.max(now)
        } else {
            now
        };
        due = Some(due.map_or(at, |current| current.min(at)));
    }
    Ok(due)
}

/// At-most-once delivery to one connection edge: matching events go to
/// the node-local inbox, the watermark advances REGARDLESS of outcome
/// (a connection edge never retries, it misses what it misses), and a
/// refusal prunes the edge (Gone or Overflow both mean the connection
/// is not coming back for these events). No registry on this runtime
/// means nothing to deliver to, which is the same prune.
fn deliver_connection_edge(
    home: &std::sync::Arc<crate::objects::ObjectHome>,
    edge: &Edge,
    registry: Option<&crate::connections::ConnectionRegistry>,
) {
    let events = match home.with_storage(|storage| events_after(storage, &edge.topic, edge.cursor))
    {
        Ok(events) => events,
        Err(error) => {
            actias_common::tracing::warn!(%error, "stream pump could not read events");
            return;
        }
    };
    let Some(last) = events.last().map(|event| event.seq) else {
        let head = home.with_storage(head_seq).unwrap_or(edge.cursor);
        let _ = home.with_storage(|storage| advance_cursor(storage, edge.id, head));
        return;
    };

    let mut pruned = false;
    if let (Some(registry), Some(connection_id)) = (registry, edge.connection.as_deref()) {
        for event in &events {
            if !filter_matches(edge.filter.as_ref(), &event.data) {
                continue;
            }
            let item = crate::connections::InboxItem::Event {
                topic: event.topic.clone(),
                from_class: event.from_class.clone(),
                from_name: event.from_name.clone(),
                data: event.data.clone(),
            };
            if let Err(refused) = registry.deliver(connection_id, item) {
                actias_common::tracing::debug!(?refused, connection_id, "connection edge pruned");
                pruned = true;
                break;
            }
        }
    } else {
        pruned = true;
    }

    if pruned {
        let _ = home.with_storage(|storage| prune_edge(storage, edge.id));
    } else {
        let _ = home.with_storage(|storage| advance_cursor(storage, edge.id, last));
    }
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

    let registry = runtime
        .app_data_ref::<std::sync::Arc<crate::connections::ConnectionRegistry>>()
        .map(|registry| registry.clone());

    for edge in edges {
        if edge.cursor >= head {
            continue;
        }
        if edge.kind == "connection" {
            deliver_connection_edge(home, &edge, registry.as_deref());
            continue;
        }
        if edge.next_at > now {
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
            publishes = { "news", "noise", private = "self" },
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
            -- Pure-Luau probe, no async anywhere: can a NATIVE yield
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
        }

        on "fetch" (function(request)
            -- The upgrade shape production uses: the program is a
            -- boot-compiled closure handed to request:upgrade, and the
            -- identity is minted from an instance handle.
            if request.upgrade and request.wants_forward then
                -- The one-stream socket, written out: the app decides
                -- the wire shape (no stdlib forwarder exists).
                return request:upgrade(function(sock)
                    sock:follow(Hub("town"), "news")
                    sock:each(function(item)
                        if item.kind == "event" then
                            sock:send({ topic = item.event.topic,
                                        from = item.event.from.id,
                                        kind = item.event.data.kind })
                        end
                    end)
                end, Reader("ada"))
            end
            if request.upgrade then
                return request:upgrade(function(sock)
                    sock:follow(Hub("town"), "news")
                    sock:each(function(item)
                        if item.kind == "event" then
                            sock:send({ kind = "event", topic = item.event.topic,
                                        from = item.event.from.id })
                        elseif item.kind == "frame" then
                            sock:send({ kind = "frame", echo = item.data.hello })
                            return true
                        end
                    end)
                end, Reader("ada"))
            end
            return { body = "ok" }
        end)
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
        town_router_with(dir, flaky, Arc::default())
    }

    /// Same placement with a shared connection registry, so connection
    /// edges deliver into test-held inboxes.
    fn town_router_with(
        dir: std::path::PathBuf,
        flaky: bool,
        registry: Arc<crate::connections::ConnectionRegistry>,
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

        // The bundle's class table declares publishes, but the STORED
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
                }),
            }),
            ..Default::default()
        };
        let prepared =
            Arc::new(PreparedRevision::prepare(Script::default(), revision).expect("prepares"));
        let channel = tonic::transport::Channel::from_static("http://127.0.0.1:1").connect_lazy();
        let runtime = ActiasRuntime::new(
            prepared,
            KvServiceClient::new(channel),
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
                    "class": "Hub", "name": "town", "method": "admit",
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

    #[tokio::test(flavor = "multi_thread")]
    async fn a_connection_program_follows_pulls_and_sends() {
        use crate::connections::OutboundFrame;
        use crate::extensions::sockets::{PendingUpgrade, SockShared, run_connection};

        let dir = tempfile::tempdir().expect("tempdir");
        let connections: Arc<crate::connections::ConnectionRegistry> = Arc::default();
        let router = town_router_with(dir.path().to_path_buf(), false, connections.clone());

        call(
            &router,
            "Hub",
            "town",
            "admit",
            vec![serde_json::json!("ada")],
        )
        .await
        .expect("admits");

        // The bridge's transport half, hand-built: registered inbox in,
        // outbound frames out; no websocket anywhere in worker-core.
        let (inbox_tx, inbox_rx) = crate::connections::inbox();
        let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<OutboundFrame>(16);
        connections.register("conn#s1", inbox_tx.clone());

        // A fresh vm from the same bundle plays the surviving request
        // vm, and the upgrade rides the script's own fetch handler:
        // arm the request, run the listener, take the parked pending.
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
        let shared = SockShared::new(
            "conn#s1".to_owned(),
            "Reader".to_owned(),
            "ada".to_owned(),
            inbox_rx,
            out_tx,
            router.clone(),
        );

        let drive = tokio::spawn(async move { run_connection(&runtime, pending, shared).await });

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

        // The program returned: polite unfollow severs the edge and the
        // bridge is told to close the wire.
        drive
            .await
            .expect("drive joins")
            .expect("the program ran cleanly");
        let closing = tokio::time::timeout(std::time::Duration::from_secs(4), out_rx.recv())
            .await
            .expect("the close reaches the wire side")
            .expect("outbound open");
        assert_eq!(closing, OutboundFrame::Close);
        let after = call(&router, "Hub", "town", "audience", vec![])
            .await
            .expect("audience answers")
            .as_i64()
            .unwrap_or(-1);
        assert_eq!(after, 0, "run_connection unfollowed on the way out");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_followers_read_answers_read_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("hub.db");
        {
            let mut storage = crate::storage::SqliteStorage::open(&file).expect("opens");
            // Follow FIRST: a fresh edge starts at head (no backfill),
            // so lag only accrues for events published after it.
            crate::streams::upsert_edge(
                &mut storage,
                "object",
                "Reader",
                "ada",
                None,
                "news",
                None,
            )
            .expect("edges");
            crate::streams::upsert_edge(
                &mut storage,
                "connection",
                "Reader",
                "ada",
                Some("conn#7"),
                "news",
                Some(&serde_json::json!({ "kind": "sport" })),
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

    /// Pins the Luau restriction the seventeenth revision rests on: a
    /// NATIVE coroutine.yield cannot cross a generic-for iterator call
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

    /// The written-out one-stream socket (the app owns the wire
    /// shape; no stdlib forwarder exists after owner review).
    #[tokio::test(flavor = "multi_thread")]
    async fn a_one_stream_program_pushes_shaped_frames() {
        use crate::connections::OutboundFrame;
        use crate::extensions::sockets::{PendingUpgrade, SockShared, run_connection};

        let dir = tempfile::tempdir().expect("tempdir");
        let connections: Arc<crate::connections::ConnectionRegistry> = Arc::default();
        let router = town_router_with(dir.path().to_path_buf(), false, connections.clone());

        call(
            &router,
            "Hub",
            "town",
            "admit",
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
        let shared = SockShared::new(
            "conn#f1".to_owned(),
            pending.class.clone(),
            pending.name.clone(),
            inbox_rx,
            out_tx,
            router.clone(),
        );
        let drive = tokio::spawn(async move { run_connection(&runtime, pending, shared).await });

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

        // Closing the wire ends the loop: Closed drains, the program
        // returns, edges sever.
        inbox_tx
            .push(crate::connections::InboxItem::Closed)
            .expect("close lands");
        drive
            .await
            .expect("drive joins")
            .expect("the program ran cleanly");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn connection_edges_deliver_at_most_once_and_prune() {
        let dir = tempfile::tempdir().expect("tempdir");
        let connections: Arc<crate::connections::ConnectionRegistry> = Arc::default();
        let router = town_router_with(dir.path().to_path_buf(), false, connections.clone());

        call(
            &router,
            "Hub",
            "town",
            "admit",
            vec![serde_json::json!("ada")],
        )
        .await
        .expect("admits");

        let (tx, mut rx) = crate::connections::inbox();
        connections.register("conn#1", tx);

        // The device follows AS Reader/ada via conn#1: exactly the
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

        // The wire goes away; the NEXT delivery prunes the edge
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
            KvServiceClient::new(channel),
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
            "admit",
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
