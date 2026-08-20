//! The `__queue` platform class: a durable message queue whose sqlite is
//! the message store and whose alarm loop is the delivery loop.
//!
//! `send` appends and arms an immediate alarm; the alarm delivers due
//! messages to the script's `on "queue:<name>"` listener one at a time. A
//! refused delivery retries with exponential backoff; a message that
//! exhausts its attempts moves to the dead-letter table instead of
//! blocking the queue. Messages ride the call's transaction, ship with
//! the file, and survive takeover like any other object row.

use serde::Serialize;

use crate::extensions::objects::{QUEUE_CLASS, unix_now_ms};
use crate::runtime::ActiasRuntime;

/// Delivery limits, worker configuration rather than constants: operators
/// tune them the way they tune sweep timings, and tests compress them.
#[derive(Clone)]
pub struct QueuePolicy {
    /// Deliveries attempted before a message dead-letters.
    pub max_attempts: i64,
    /// First retry delay; each further attempt doubles it, capped at an
    /// hour.
    pub backoff_base_ms: i64,
}

impl Default for QueuePolicy {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            backoff_base_ms: 2000,
        }
    }
}

/// What the dashboard and `stats` calls read; plain data by design.
#[derive(Serialize)]
pub struct Stats {
    pub depth: i64,
    pub oldest_pending: Option<i64>,
    pub dead_letters: i64,
}

/// Messages awaiting delivery. Plain rowid ordering is FIFO among live
/// rows, which is the only ordering delivery observes.
const CREATE_MESSAGES: &str = "CREATE TABLE IF NOT EXISTS __actias_queue_messages (
        id INTEGER PRIMARY KEY,
        payload TEXT NOT NULL,
        attempts INTEGER NOT NULL DEFAULT 0,
        next_at INTEGER NOT NULL,
        enqueued_at INTEGER NOT NULL
    )";

/// Messages that exhausted their attempts; kept for inspection, never
/// redelivered by the platform.
const CREATE_DEAD: &str = "CREATE TABLE IF NOT EXISTS __actias_queue_dead (
        id INTEGER,
        payload TEXT,
        attempts INTEGER,
        enqueued_at INTEGER,
        died_at INTEGER
    )";

/// How many due messages one alarm firing works through before re-arming.
const DELIVERY_BATCH: i64 = 16;

/// The inspector's journal: every message state change, ring-buffered.
const CREATE_EVENTS: &str = "CREATE TABLE IF NOT EXISTS __actias_queue_events (
        seq INTEGER PRIMARY KEY,
        at INTEGER NOT NULL,
        kind TEXT NOT NULL,
        detail TEXT NOT NULL
    )";

/// Journal rows kept; older ones fall off the ring.
const EVENT_RING: i64 = 300;

/// Appends one journal row and trims the ring; called inside the call's
/// transaction, so events and the state they describe commit together.
fn record_event(
    connection: &mut rusqlite::Connection,
    kind: &str,
    detail: &str,
) -> Result<(), String> {
    connection
        .execute(CREATE_EVENTS, [])
        .map_err(|e| e.to_string())?;
    connection
        .execute(
            "INSERT INTO __actias_queue_events (at, kind, detail) VALUES (?, ?, ?)",
            rusqlite::params![unix_now_ms(), kind, detail],
        )
        .map_err(|e| e.to_string())?;
    connection
        .execute(
            "DELETE FROM __actias_queue_events WHERE seq <=              (SELECT MAX(seq) FROM __actias_queue_events) - ?",
            rusqlite::params![EVENT_RING],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Routes one `__queue` method call.
///
/// # Errors
/// Returns the user-safe text of whatever failed; unknown methods read
/// like a missing method on any class.
pub(crate) async fn dispatch(
    runtime: &ActiasRuntime,
    context: &super::PlatformContext<'_>,
    call: &super::Call,
) -> Result<serde_json::Value, String> {
    // Schema setup happens once per file: the version cell is the record,
    // and the day a second schema version exists, its migration belongs
    // right here.
    context.home.with_storage(|storage| {
        if !storage.is_fresh()? {
            return Ok(());
        }
        let connection = storage.platform();
        connection
            .execute(CREATE_MESSAGES, [])
            .map_err(|e| e.to_string())?;
        connection
            .execute(CREATE_DEAD, [])
            .map_err(|e| e.to_string())?;
        storage.mark_initialized()
    })?;

    match call.method.as_str() {
        "send" => send(
            context,
            call.args
                .first()
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        ),
        "alarm" => deliver(runtime, context).await,
        "stats" => stats(context),
        "events" => events(
            context,
            call.args
                .first()
                .and_then(|value| value.as_i64())
                .unwrap_or(0),
        ),
        other => Err(format!(
            "Object class '{QUEUE_CLASS}' has no method '{other}'."
        )),
    }
}

/// Appends one message and arms an immediate delivery alarm.
fn send(
    context: &super::PlatformContext<'_>,
    payload: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let text = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    let now = unix_now_ms();

    context.home.with_storage(|storage| {
        let connection = storage.platform();
        connection
            .execute(
                "INSERT INTO __actias_queue_messages (payload, next_at, enqueued_at) VALUES (?, ?, ?)",
                rusqlite::params![text, now, now],
            )
            .map_err(|e| e.to_string())?;
        record_event(connection, "enqueued", &text.chars().take(120).collect::<String>())?;
        Ok(())
    })?;

    super::set_alarm(context, QUEUE_CLASS, 0)?;
    Ok(serde_json::Value::Bool(true))
}

/// One due message as delivery reads it.
struct Due {
    id: i64,
    payload: String,
    attempts: i64,
}

/// Delivers due messages to the `on "queue:<name>"` listener, applying
/// the per-message verdict, then re-arms for the earliest remaining
/// message. The storage borrow is never held across the listener await.
async fn deliver(
    runtime: &ActiasRuntime,
    context: &super::PlatformContext<'_>,
) -> Result<serde_json::Value, String> {
    let policy = context.home.queue_policy();
    let event = format!("queue:{}", context.name);

    let due = context.home.with_storage(|storage| {
        let mut statement = storage
            .platform()
            .prepare(
                "SELECT id, payload, attempts FROM __actias_queue_messages \
                 WHERE next_at <= ? ORDER BY id LIMIT ?",
            )
            .map_err(|e| e.to_string())?;
        let due = statement
            .query_map(rusqlite::params![unix_now_ms(), DELIVERY_BATCH], |row| {
                Ok(Due {
                    id: row.get(0)?,
                    payload: row.get(1)?,
                    attempts: row.get(2)?,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(due)
    })?;

    for message in due {
        // A payload that no longer parses is corrupt storage, an expected
        // input: it delivers as null rather than wedging the queue.
        let payload = serde_json::from_str(&message.payload).unwrap_or(serde_json::Value::Null);
        let delivered = super::fire_listener(runtime, &event, &payload).await;

        context.home.with_storage(|storage| {
            let connection = storage.platform();
            if delivered {
                connection
                    .execute(
                        "DELETE FROM __actias_queue_messages WHERE id = ?",
                        rusqlite::params![message.id],
                    )
                    .map_err(|e| e.to_string())?;
                record_event(
                    connection,
                    "delivered",
                    &message.payload.chars().take(120).collect::<String>(),
                )?;
            } else if message.attempts + 1 >= policy.max_attempts {
                connection
                    .execute(
                        "INSERT INTO __actias_queue_dead \
                         SELECT id, payload, attempts + 1, enqueued_at, ? \
                         FROM __actias_queue_messages WHERE id = ?",
                        rusqlite::params![unix_now_ms(), message.id],
                    )
                    .map_err(|e| e.to_string())?;
                connection
                    .execute(
                        "DELETE FROM __actias_queue_messages WHERE id = ?",
                        rusqlite::params![message.id],
                    )
                    .map_err(|e| e.to_string())?;
                record_event(
                    connection,
                    "dead-lettered",
                    &format!("after {} attempts", message.attempts + 1),
                )?;
            } else {
                let backoff = (policy.backoff_base_ms << message.attempts).min(3_600_000);
                connection
                    .execute(
                        "UPDATE __actias_queue_messages \
                         SET attempts = attempts + 1, next_at = ? WHERE id = ?",
                        rusqlite::params![unix_now_ms() + backoff, message.id],
                    )
                    .map_err(|e| e.to_string())?;
                record_event(
                    connection,
                    "retried",
                    &format!("attempt {}, next in {}ms", message.attempts + 1, backoff),
                )?;
            }
            Ok(())
        })?;
    }

    let earliest: Option<i64> = context.home.with_storage(|storage| {
        storage
            .platform()
            .query_row(
                "SELECT MIN(next_at) FROM __actias_queue_messages",
                [],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())
    })?;

    if let Some(at) = earliest {
        super::set_alarm(context, QUEUE_CLASS, at - unix_now_ms())?;
    }

    Ok(serde_json::Value::Null)
}

/// The queue's numbers, dispatched as a method today and reusable by any
/// read path that can open the file (a snapshot, a replica, an api
/// endpoint) without dispatching at all.
pub fn read_stats(storage: &mut crate::storage::SqliteStorage) -> Result<Stats, String> {
    // A file that predates the schema (a fresh object, an old snapshot, a
    // replica) reads as an empty queue rather than an error; this also
    // keeps the accessor safe on read-only connections, where issuing DDL
    // would fail.
    let connection = storage.platform();
    let missing = |error: &rusqlite::Error| error.to_string().contains("no such table");

    let (depth, oldest_pending) = match connection.query_row(
        "SELECT COUNT(*), MIN(enqueued_at) FROM __actias_queue_messages",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ) {
        Ok(row) => row,
        Err(error) if missing(&error) => (0, None),
        Err(error) => return Err(error.to_string()),
    };
    let dead_letters =
        match connection.query_row("SELECT COUNT(*) FROM __actias_queue_dead", [], |row| {
            row.get(0)
        }) {
            Ok(count) => count,
            Err(error) if missing(&error) => 0,
            Err(error) => return Err(error.to_string()),
        };

    Ok(Stats {
        depth,
        oldest_pending,
        dead_letters,
    })
}

/// The `stats` method: [`read_stats`] over this object's own storage.
fn stats(context: &super::PlatformContext<'_>) -> Result<serde_json::Value, String> {
    let stats = context.home.with_storage(read_stats)?;
    serde_json::to_value(stats).map_err(|e| e.to_string())
}

/// One journal row, plain data for the inspector.
#[derive(Serialize)]
pub struct QueueEvent {
    pub seq: i64,
    pub at: i64,
    pub kind: String,
    pub detail: String,
}

/// Journal rows after `since`, oldest first; reusable by any read path
/// that can open the file.
pub fn read_events(
    storage: &mut crate::storage::SqliteStorage,
    since: i64,
) -> Result<Vec<QueueEvent>, String> {
    let connection = storage.platform();
    connection
        .execute(CREATE_EVENTS, [])
        .map_err(|e| e.to_string())?;
    let mut statement = connection
        .prepare(
            "SELECT seq, at, kind, detail FROM __actias_queue_events \
             WHERE seq > ? ORDER BY seq LIMIT 200",
        )
        .map_err(|e| e.to_string())?;
    let events = statement
        .query_map(rusqlite::params![since], |row| {
            Ok(QueueEvent {
                seq: row.get(0)?,
                at: row.get(1)?,
                kind: row.get(2)?,
                detail: row.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(events)
}

/// The `events` method: [`read_events`] over this object's own storage.
fn events(context: &super::PlatformContext<'_>, since: i64) -> Result<serde_json::Value, String> {
    let events = context
        .home
        .with_storage(|storage| read_events(storage, since))?;
    serde_json::to_value(events).map_err(|e| e.to_string())
}
