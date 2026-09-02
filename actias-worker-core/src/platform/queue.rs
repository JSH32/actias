//! The `__queue` platform class: a durable message queue whose sqlite is
//! the message store and whose alarm loop is the delivery loop.
//!
//! `send` appends and arms an immediate alarm; the alarm delivers due
//! messages to the script's `on "queue:<name>"` listener one at a time. A
//! refused delivery retries with exponential backoff; a message that
//! exhausts its attempts moves to the dead-letter table instead of
//! blocking the queue, where `retry_dead`/`retry_message` can requeue it
//! and `drop_message` discards it. Messages ride the call's transaction,
//! ship with the file, and survive takeover like any other object row.
//!
//! Every state change is journaled into a ring table committed with the
//! state it describes; the detail column is json (message id, payload
//! preview, producer, per-attempt error), which is what the dashboard's
//! inspector renders.

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
    /// Every message still queued, the in-flight ones included.
    pub depth: i64,
    /// Messages due now, in delivery's hands.
    pub in_flight: i64,
    pub oldest_pending: Option<i64>,
    pub dead_letters: i64,
}

/// One message row as the inspector's table shows it.
#[derive(Serialize)]
pub struct Message {
    pub id: i64,
    /// pending, in-flight or dead; delivered rows live in the journal.
    pub state: String,
    pub attempts: i64,
    pub preview: String,
    /// The whole payload text, for the inspector's drawer; queues are
    /// quota-small by design, so rows carry it whole.
    pub payload: String,
    pub size: i64,
    pub enqueued_ms: i64,
    /// When delivery next picks it up; absent for dead letters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_ms: Option<i64>,
    /// When it dead-lettered; absent for live rows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub died_ms: Option<i64>,
}

/// Table names, for existence probes; each must match its DDL below.
const MESSAGES_TABLE: &str = "__actias_queue_messages";
const DEAD_TABLE: &str = "__actias_queue_dead";
const EVENTS_TABLE: &str = "__actias_queue_events";

/// The queue schema's version, stamped in the file's version cell.
/// Version 1 predates AUTOINCREMENT ids: rowids could reuse after a
/// delete, so one journal id named several generations of messages.
/// Version 2 rebuilds the table so an id names exactly one message for
/// the file's whole life.
const SCHEMA_VERSION: i64 = 2;

/// Messages awaiting delivery. Id ordering is FIFO among live rows,
/// which is the only ordering delivery observes; AUTOINCREMENT keeps
/// every id unique forever, which is what makes journal entries and the
/// retry/drop controls unambiguous.
const CREATE_MESSAGES: &str = "CREATE TABLE IF NOT EXISTS __actias_queue_messages (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        payload TEXT NOT NULL,
        attempts INTEGER NOT NULL DEFAULT 0,
        next_at INTEGER NOT NULL,
        enqueued_at INTEGER NOT NULL
    )";

/// Messages that exhausted their attempts; kept for inspection and manual
/// requeueing, never redelivered by the platform on its own.
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

/// Longest payload prefix a journal row or message listing carries.
const PREVIEW_CHARS: usize = 120;

/// Longest payload a delivered event retains in the journal, so the
/// inspector can read what was delivered after the message row is
/// gone. Generous for real messages, a cap for pathological ones: the
/// ring holds EVENT_RING of these inside the queue's own file.
const DELIVERED_PAYLOAD_CHARS: usize = 32 * 1024;

fn preview(payload: &str) -> String {
    payload.chars().take(PREVIEW_CHARS).collect()
}

/// Appends one journal row and trims the ring; called inside the call's
/// transaction, so events and the state they describe commit together.
/// The detail is json, the shape the inspector renders.
fn record_event(
    connection: &mut rusqlite::Connection,
    kind: &str,
    detail: &serde_json::Value,
) -> Result<(), String> {
    connection
        .execute(CREATE_EVENTS, [])
        .map_err(|e| e.to_string())?;
    connection
        .execute(
            "INSERT INTO __actias_queue_events (at, kind, detail) VALUES (?, ?, ?)",
            rusqlite::params![unix_now_ms(), kind, detail.to_string()],
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
    // Schema setup happens once per file; the version cell is the record
    // and carries this file forward when the schema moves.
    context.home.with_storage(|storage| {
        let version = storage.schema_version()?;
        if version >= SCHEMA_VERSION {
            return Ok(());
        }
        let connection = storage.platform();
        if version == 0 {
            connection
                .execute(CREATE_MESSAGES, [])
                .map_err(|e| e.to_string())?;
            connection
                .execute(CREATE_DEAD, [])
                .map_err(|e| e.to_string())?;
        } else {
            // v1 -> v2: rebuild messages so ids never reuse. Rows carry
            // over with their ids, and AUTOINCREMENT resumes past the
            // highest one; runs inside the call's transaction like any
            // other platform write.
            connection
                .execute_batch(&format!(
                    "ALTER TABLE __actias_queue_messages RENAME TO __actias_queue_messages_v1;
                     {CREATE_MESSAGES};
                     INSERT INTO __actias_queue_messages SELECT * FROM __actias_queue_messages_v1;
                     DROP TABLE __actias_queue_messages_v1;"
                ))
                .map_err(|e| e.to_string())?;
        }
        storage.set_schema_version(SCHEMA_VERSION)
    })?;

    match call.method.as_str() {
        "send" => send(
            context,
            call.args
                .first()
                .cloned()
                .unwrap_or(serde_json::Value::Null),
            call.caller.as_ref(),
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
        "messages" => messages(context),
        "retry_dead" => retry_dead(context, None),
        "retry_message" => retry_dead(context, require_id(call)?.into()),
        "drop_message" => drop_message(context, require_id(call)?),
        other => Err(format!(
            "Object class '{QUEUE_CLASS}' has no method '{other}'."
        )),
    }
}

/// The message id argument the retry/drop controls take.
fn require_id(call: &super::Call) -> Result<i64, String> {
    call.args
        .first()
        .and_then(|value| value.as_i64())
        .ok_or_else(|| "The message id must be a number.".to_owned())
}

/// Appends one message and arms an immediate delivery alarm. The journal
/// row carries the producer when the router knew the caller.
fn send(
    context: &super::PlatformContext<'_>,
    payload: serde_json::Value,
    caller: Option<&super::Caller>,
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
        let id = connection.last_insert_rowid();
        record_event(
            connection,
            "enqueued",
            &serde_json::json!({
                "id": id,
                "preview": preview(&text),
                "size": text.len(),
                "producer_script": caller.map(|c| c.script.as_str()),
                "producer_revision": caller.map(|c| c.revision.as_str()),
            }),
        )?;
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
        let verdict = super::fire_listener(runtime, &event, &payload).await;
        let attempt = message.attempts + 1;

        context.home.with_storage(|storage| {
            let connection = storage.platform();
            match &verdict {
                Ok(()) => {
                    connection
                        .execute(
                            "DELETE FROM __actias_queue_messages WHERE id = ?",
                            rusqlite::params![message.id],
                        )
                        .map_err(|e| e.to_string())?;
                    record_event(
                        connection,
                        "delivered",
                        &serde_json::json!({
                            "id": message.id,
                            "attempt": attempt,
                            "payload": message
                                .payload
                                .chars()
                                .take(DELIVERED_PAYLOAD_CHARS)
                                .collect::<String>(),
                            "payload_truncated":
                                message.payload.chars().count() > DELIVERED_PAYLOAD_CHARS,
                        }),
                    )?;
                }
                Err(error) if attempt >= policy.max_attempts => {
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
                        &serde_json::json!({
                            "id": message.id,
                            "attempt": attempt,
                            "error": error,
                        }),
                    )?;
                }
                Err(error) => {
                    let backoff = (policy.backoff_base_ms << message.attempts).min(3_600_000);
                    let next_ms = unix_now_ms() + backoff;
                    connection
                        .execute(
                            "UPDATE __actias_queue_messages \
                             SET attempts = attempts + 1, next_at = ? WHERE id = ?",
                            rusqlite::params![next_ms, message.id],
                        )
                        .map_err(|e| e.to_string())?;
                    record_event(
                        connection,
                        "retried",
                        &serde_json::json!({
                            "id": message.id,
                            "attempt": attempt,
                            "error": error,
                            "next_ms": next_ms,
                        }),
                    )?;
                }
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

/// Requeues dead letters: all of them, or one by id. Requeued rows start
/// their attempts over and become new messages (new ids), which the
/// journal records.
fn retry_dead(
    context: &super::PlatformContext<'_>,
    id: Option<i64>,
) -> Result<serde_json::Value, String> {
    let count = context.home.with_storage(|storage| {
        let connection = storage.platform();
        let now = unix_now_ms();

        let (filter, params) = match id {
            Some(id) => (" WHERE id = ?", vec![now, id]),
            None => ("", vec![now]),
        };
        let moved = connection
            .execute(
                &format!(
                    "INSERT INTO __actias_queue_messages (payload, attempts, next_at, enqueued_at) \
                     SELECT payload, 0, ?, enqueued_at FROM __actias_queue_dead{filter}"
                ),
                rusqlite::params_from_iter(params.iter()),
            )
            .map_err(|e| e.to_string())?;
        if moved > 0 {
            let delete_params: Vec<i64> = id.into_iter().collect();
            connection
                .execute(
                    &format!("DELETE FROM __actias_queue_dead{filter}"),
                    rusqlite::params_from_iter(delete_params.iter()),
                )
                .map_err(|e| e.to_string())?;
            record_event(
                connection,
                "requeued",
                &serde_json::json!({ "count": moved, "id": id }),
            )?;
        }
        Ok(moved as i64)
    })?;

    if count > 0 {
        super::set_alarm(context, QUEUE_CLASS, 0)?;
    }
    Ok(serde_json::json!(count))
}

/// Discards one message, live or dead; the journal records the drop.
fn drop_message(
    context: &super::PlatformContext<'_>,
    id: i64,
) -> Result<serde_json::Value, String> {
    context.home.with_storage(|storage| {
        let connection = storage.platform();
        let live = connection
            .execute(
                "DELETE FROM __actias_queue_messages WHERE id = ?",
                rusqlite::params![id],
            )
            .map_err(|e| e.to_string())?;
        let dead = connection
            .execute(
                "DELETE FROM __actias_queue_dead WHERE id = ?",
                rusqlite::params![id],
            )
            .map_err(|e| e.to_string())?;
        if live + dead > 0 {
            record_event(connection, "dropped", &serde_json::json!({ "id": id }))?;
        }
        Ok(serde_json::json!(live + dead > 0))
    })
}

/// The queue's numbers, dispatched as a method today and reusable by any
/// read path that can open the file (a snapshot, a replica, an api
/// endpoint) without dispatching at all. A file that predates the schema
/// (a fresh object, an old snapshot, a replica) reads as an empty queue,
/// decided by probing for the tables rather than classifying errors, so
/// the accessor is safe on read-only connections too.
///
/// # Errors
/// Returns SQLite's message.
pub fn read_stats(storage: &mut crate::storage::SqliteStorage) -> Result<Stats, String> {
    let (depth, in_flight, oldest_pending) = if storage.table_exists(MESSAGES_TABLE)? {
        storage
            .platform()
            .query_row(
                "SELECT COUNT(*), COUNT(*) FILTER (WHERE next_at <= ?), MIN(enqueued_at) \
                 FROM __actias_queue_messages",
                rusqlite::params![unix_now_ms()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|e| e.to_string())?
    } else {
        (0, 0, None)
    };

    let dead_letters = if storage.table_exists(DEAD_TABLE)? {
        storage
            .platform()
            .query_row("SELECT COUNT(*) FROM __actias_queue_dead", [], |row| {
                row.get(0)
            })
            .map_err(|e| e.to_string())?
    } else {
        0
    };

    Ok(Stats {
        depth,
        in_flight,
        oldest_pending,
        dead_letters,
    })
}

/// The `stats` method: [`read_stats`] over this object's own storage.
fn stats(context: &super::PlatformContext<'_>) -> Result<serde_json::Value, String> {
    let stats = context.home.with_storage(read_stats)?;
    serde_json::to_value(stats).map_err(|e| e.to_string())
}

/// Live and dead message rows for the inspector's table, newest first;
/// delivered messages live in the journal, not here. Reusable by any read
/// path that can open the file.
///
/// # Errors
/// Returns SQLite's message.
pub fn read_messages(storage: &mut crate::storage::SqliteStorage) -> Result<Vec<Message>, String> {
    let mut rows = Vec::new();
    let now = unix_now_ms();

    if storage.table_exists(MESSAGES_TABLE)? {
        let connection = storage.platform();
        let mut statement = connection
            .prepare(
                "SELECT id, payload, attempts, next_at, enqueued_at \
                 FROM __actias_queue_messages ORDER BY id DESC LIMIT 200",
            )
            .map_err(|e| e.to_string())?;
        let live = statement
            .query_map([], |row| {
                let payload: String = row.get(1)?;
                let next_at: i64 = row.get(3)?;
                Ok(Message {
                    id: row.get(0)?,
                    state: if next_at <= now {
                        "in-flight"
                    } else {
                        "pending"
                    }
                    .to_owned(),
                    attempts: row.get(2)?,
                    preview: preview(&payload),
                    size: payload.len() as i64,
                    payload,
                    enqueued_ms: row.get(4)?,
                    next_ms: Some(next_at),
                    died_ms: None,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        rows.extend(live);
    }

    if storage.table_exists(DEAD_TABLE)? {
        let connection = storage.platform();
        let mut statement = connection
            .prepare(
                "SELECT id, payload, attempts, enqueued_at, died_at \
                 FROM __actias_queue_dead ORDER BY id DESC LIMIT 200",
            )
            .map_err(|e| e.to_string())?;
        let dead = statement
            .query_map([], |row| {
                let payload: String = row.get(1)?;
                Ok(Message {
                    id: row.get(0)?,
                    state: "dead".to_owned(),
                    attempts: row.get(2)?,
                    preview: preview(&payload),
                    size: payload.len() as i64,
                    payload,
                    enqueued_ms: row.get(3)?,
                    next_ms: None,
                    died_ms: row.get(4)?,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        rows.extend(dead);
    }

    rows.sort_by(|a, b| b.enqueued_ms.cmp(&a.enqueued_ms).then(b.id.cmp(&a.id)));
    Ok(rows)
}

/// The `messages` method: [`read_messages`] over this object's own storage.
fn messages(context: &super::PlatformContext<'_>) -> Result<serde_json::Value, String> {
    let messages = context.home.with_storage(read_messages)?;
    serde_json::to_value(messages).map_err(|e| e.to_string())
}

/// One journal row, plain data for the inspector; the detail is json.
#[derive(Serialize)]
pub struct QueueEvent {
    pub seq: i64,
    pub at: i64,
    pub kind: String,
    pub detail: serde_json::Value,
}

/// Journal rows after `since`, oldest first. A file without the journal
/// yet reads as empty, probed rather than created: writes own the DDL,
/// reads never issue any.
pub fn read_events(
    storage: &mut crate::storage::SqliteStorage,
    since: i64,
) -> Result<Vec<QueueEvent>, String> {
    if !storage.table_exists(EVENTS_TABLE)? {
        return Ok(Vec::new());
    }
    let connection = storage.platform();
    let mut statement = connection
        .prepare(
            "SELECT seq, at, kind, detail FROM __actias_queue_events \
             WHERE seq > ? ORDER BY seq LIMIT 200",
        )
        .map_err(|e| e.to_string())?;
    let events = statement
        .query_map(rusqlite::params![since], |row| {
            let detail: String = row.get(3)?;
            Ok(QueueEvent {
                seq: row.get(0)?,
                at: row.get(1)?,
                kind: row.get(2)?,
                // Rows written before the json journal read as a string.
                detail: serde_json::from_str(&detail).unwrap_or(serde_json::Value::String(detail)),
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
