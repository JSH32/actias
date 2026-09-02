//! The follower's receive cursor per publisher and topic: what makes
//! redelivery a skip rather than a repeat.

use super::*;

/// The follower's own high-water mark for one stream: with the cursor
/// beside the handler's writes, a redelivered event skips instead of
/// re-running, and both commit together.
pub fn receive_cursor(
    storage: &mut SqliteStorage,
    publisher: &str,
    topic: &str,
) -> Result<i64, String> {
    let connection = storage.platform();
    connection
        .execute(CREATE_RECEIVE_CURSORS, [])
        .map_err(|e| e.to_string())?;
    connection
        .query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM __actias_receive_cursors              WHERE publisher = ? AND topic = ?",
            rusqlite::params![publisher, topic],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())
}

pub fn advance_receive_cursor(
    storage: &mut SqliteStorage,
    publisher: &str,
    topic: &str,
    seq: i64,
) -> Result<(), String> {
    let connection = storage.platform();
    connection
        .execute(CREATE_RECEIVE_CURSORS, [])
        .map_err(|e| e.to_string())?;
    connection
        .execute(
            "INSERT INTO __actias_receive_cursors (publisher, topic, seq) VALUES (?, ?, ?)              ON CONFLICT (publisher, topic) DO UPDATE SET seq = MAX(seq, excluded.seq)",
            rusqlite::params![publisher, topic, seq],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}
