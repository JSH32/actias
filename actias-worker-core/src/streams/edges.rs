//! The edge table: who follows what, from where, how far they have
//! heard, and what a failure does to the edge.

use super::*;

/// One edge row, as the pump and the `followers` verb read it.
#[derive(Clone, Debug)]
pub struct Edge {
    pub id: i64,
    pub kind: String,
    pub class: String,
    pub name: String,
    /// Connection edges only: the node-local connection this edge
    /// delivers to; the identity above is who that connection speaks as.
    pub connection: Option<String>,
    pub topic: String,
    pub filter: Option<serde_json::Value>,
    pub cursor: i64,
    pub attempts: i64,
    pub next_at: i64,
    /// Where the follower lives; connection edges deliver there. Empty
    /// or absent means this node (edges from before homes were
    /// recorded, and single-node installs).
    pub node: Option<String>,
}

/// One accepted follow, as [`upsert_edge`] records it.
pub struct EdgeSpec<'a> {
    pub kind: &'a str,
    pub class: &'a str,
    pub name: &'a str,
    /// Present for connection edges only.
    pub connection_id: Option<&'a str>,
    pub topic: &'a str,
    pub filter: Option<&'a serde_json::Value>,
    /// Where the follower lives; [`None`] means this node.
    pub node: Option<&'a str>,
}

/// Records (or re-records) an accepted follow: one edge per
/// (kind, class, name, topic); re-following replaces the filter and
/// resets patience, keeping the cursor so no history replays.
///
/// # Errors
/// Returns SQLite's message.
pub fn upsert_edge(storage: &mut SqliteStorage, edge: EdgeSpec<'_>) -> Result<(), String> {
    let EdgeSpec {
        kind,
        class,
        name,
        connection_id,
        topic,
        filter,
        node,
    } = edge;
    ensure_tables(storage)?;
    let filter_text = filter.map(|value| value.to_string());
    let connection = storage.platform();
    // One identity may hold the same topic once per endpoint: the
    // durable edge and each connected device are separate rows (the
    // doc's two-devices table), so connection is part of the match.
    let updated = connection
        .execute(
            "UPDATE __actias_followers SET filter = ?, attempts = 0, next_at = 0, node = ? \
             WHERE kind = ? AND class = ? AND name = ? AND topic = ? AND connection IS ?",
            rusqlite::params![filter_text, node, kind, class, name, topic, connection_id],
        )
        .map_err(|e| e.to_string())?;
    if updated == 0 {
        // A fresh edge starts at the log head: a follow never replays
        // events published before it, because history is the publisher's
        // state and reading it is an ordinary method.
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
                     (kind, class, name, connection, topic, filter, cursor, node) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![
                    kind,
                    class,
                    name,
                    connection_id,
                    topic,
                    filter_text,
                    head,
                    node
                ],
            )
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Deletes one edge by id: the pump's deliver-or-prune half for
/// connection edges, and dead-edge cleanup generally.
///
/// # Errors
/// Returns SQLite's message.
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

/// Removes one identity's edge on one topic at one endpoint; unilateral,
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
/// from a file opened read-only, so nothing here may create tables;
/// an object that never touched streams reads as empty, not an error.
/// Object-kind edges carry their lag (head minus cursor); connection
/// edges have no cursor promise and report none.
///
/// # Errors
/// Returns SQLite's message.
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
///
/// # Errors
/// Returns SQLite's message.
pub fn list_edges(storage: &mut SqliteStorage, topic: Option<&str>) -> Result<Vec<Edge>, String> {
    ensure_tables(storage)?;
    let connection = storage.platform();
    let mut statement = connection
        .prepare(
            "SELECT id, kind, class, name, connection, topic, filter, cursor, attempts, next_at, \
                    node \
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
                node: row.get(10)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut edges = Vec::new();
    for row in rows {
        edges.push(row.map_err(|e| e.to_string())?);
    }
    Ok(edges)
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
///
/// # Errors
/// Returns SQLite's message.
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
/// Forgets where these followers live: the next pump delivers them by
/// routing the follower's identity instead of batching to a node, and
/// a later re-follow records the fresh home.
pub(super) fn clear_edge_nodes(
    storage: &mut SqliteStorage,
    edge_ids: &[i64],
) -> Result<(), String> {
    for edge_id in edge_ids {
        storage
            .platform()
            .execute(
                "UPDATE __actias_followers SET node = NULL WHERE id = ?",
                rusqlite::params![edge_id],
            )
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

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
///
/// # Errors
/// Returns SQLite's message.
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
