//! The reserved tables a publisher's file carries: its event log, its
//! edges, and the receive cursors of what it follows.

use super::*;

pub(super) const CREATE_EVENTS: &str = "CREATE TABLE IF NOT EXISTS __actias_stream_events (
        seq INTEGER PRIMARY KEY AUTOINCREMENT,
        at INTEGER NOT NULL,
        from_class TEXT NOT NULL,
        from_name TEXT NOT NULL,
        topic TEXT NOT NULL,
        data TEXT NOT NULL
    )";

pub(super) const CREATE_FOLLOWERS: &str = "CREATE TABLE IF NOT EXISTS __actias_followers (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        kind TEXT NOT NULL,
        class TEXT NOT NULL,
        name TEXT NOT NULL,
        connection TEXT,
        topic TEXT NOT NULL,
        filter TEXT,
        cursor INTEGER NOT NULL DEFAULT 0,
        attempts INTEGER NOT NULL DEFAULT 0,
        next_at INTEGER NOT NULL DEFAULT 0,
        node TEXT
    )";

/// Creates the stream tables when absent; idempotent, called from every
/// verb that touches them.
///
/// # Errors
/// Returns SQLite's message.
pub fn ensure_tables(storage: &mut SqliteStorage) -> Result<(), String> {
    let connection = storage.platform();
    connection
        .execute(CREATE_EVENTS, [])
        .map_err(|e| e.to_string())?;
    connection
        .execute(CREATE_FOLLOWERS, [])
        .map_err(|e| e.to_string())?;
    // Publishers born before edges carried a home: the column arrives
    // on first touch, and the duplicate-column refusal is the
    // idempotence.
    let _ = connection.execute("ALTER TABLE __actias_followers ADD COLUMN node TEXT", []);
    Ok(())
}

pub(super) const CREATE_RECEIVE_CURSORS: &str =
    "CREATE TABLE IF NOT EXISTS __actias_receive_cursors (
        publisher TEXT NOT NULL,
        topic TEXT NOT NULL,
        seq INTEGER NOT NULL,
        PRIMARY KEY (publisher, topic)
    )";
