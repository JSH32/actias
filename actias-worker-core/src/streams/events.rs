//! The event log: append in the publisher's transaction, read back by
//! sequence.

use super::*;

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
///
/// # Errors
/// Returns SQLite's message.
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
