//! The workflow journal on object storage: one append-only table in the
//! instance's own SQLite file. Everything a run ever did or decided is a
//! row here; replay is reading it back in order. The verbs (W3) fold
//! over this; this module owns only the substrate: schema, kinds,
//! append, cursor reads.
//!
//! Entries carry a format version from day one, the cheap insurance that
//! lets a later engine (or a continuation checkpoint) replace the replay
//! tail without a table migration.

use serde::{Deserialize, Serialize};

/// The journal schema's version cell value; moves like the queue's did
/// (v1 to v2 rebuild proved the mechanism).
const SCHEMA_VERSION: i64 = 1;

/// The current entry format; stamped per row, not per file, so a tail
/// written by newer code coexists with an older head.
pub const ENTRY_FORMAT: i64 = 1;

/// Sequence order IS execution order: the mailbox serializes appends
/// structurally, and AUTOINCREMENT keeps seq unique forever even if
/// rows were ever pruned.
const CREATE_JOURNAL: &str = "CREATE TABLE IF NOT EXISTS __actias_wf_journal (
        seq INTEGER PRIMARY KEY AUTOINCREMENT,
        at INTEGER NOT NULL,
        kind TEXT NOT NULL,
        data TEXT NOT NULL,
        format INTEGER NOT NULL
    )";

/// Everything a journal row can record. The set is closed on purpose:
/// replay must understand every kind it can meet, so a new kind is a
/// format bump, never a silent addition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EntryKind {
    /// The run began: input, pinned revision id, random seed, engine.
    Started,
    /// A step is about to run its effect; the irreducible crash window
    /// opens here.
    Intent,
    /// The step's effect finished; replay returns this value instead of
    /// running the body again.
    Result,
    /// A sleep parked the run until a due time; the standard alarm row
    /// mirrors it.
    Timer,
    /// A signal arrived (or an await parked waiting for one).
    Signal,
    /// A child run was spawned; its name derives from this run's.
    Child,
    /// The run was asked to stop; propagates to children.
    Cancel,
    /// The function returned; the row holds the return value.
    Completed,
}

impl EntryKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Started => "STARTED",
            Self::Intent => "INTENT",
            Self::Result => "RESULT",
            Self::Timer => "TIMER",
            Self::Signal => "SIGNAL",
            Self::Child => "CHILD",
            Self::Cancel => "CANCEL",
            Self::Completed => "COMPLETED",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "STARTED" => Self::Started,
            "INTENT" => Self::Intent,
            "RESULT" => Self::Result,
            "TIMER" => Self::Timer,
            "SIGNAL" => Self::Signal,
            "CHILD" => Self::Child,
            "CANCEL" => Self::Cancel,
            "COMPLETED" => Self::Completed,
            _ => return None,
        })
    }
}

/// One journal row, as replay consumes it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Entry {
    pub seq: i64,
    /// Unix milliseconds the row was appended.
    pub at: i64,
    pub kind: EntryKind,
    pub data: serde_json::Value,
    pub format: i64,
}

/// Creates the journal table once per file; the version cell is the
/// record and carries the file forward when the schema moves.
///
/// # Errors
/// Returns SQLite's message.
pub fn ensure_schema(storage: &mut crate::storage::SqliteStorage) -> Result<(), String> {
    let version = storage.schema_version()?;
    if version >= SCHEMA_VERSION {
        return Ok(());
    }
    storage
        .platform()
        .execute(CREATE_JOURNAL, [])
        .map_err(|e| e.to_string())?;
    storage.set_schema_version(SCHEMA_VERSION)
}

/// Appends one entry and returns its sequence number. Appends ride the
/// current call's transaction like every platform write, so a journal
/// row commits exactly with the state it describes.
///
/// # Errors
/// Returns SQLite's message.
pub fn append(
    storage: &mut crate::storage::SqliteStorage,
    kind: EntryKind,
    data: &serde_json::Value,
) -> Result<i64, String> {
    let at = crate::extensions::objects::unix_now_ms();
    let connection = storage.platform();
    connection
        .execute(
            "INSERT INTO __actias_wf_journal (at, kind, data, format) VALUES (?, ?, ?, ?)",
            rusqlite::params![at, kind.as_str(), data.to_string(), ENTRY_FORMAT],
        )
        .map_err(|e| e.to_string())?;
    Ok(connection.last_insert_rowid())
}

/// Every entry at or after `from_seq`, in sequence order: the replay
/// read. `from_seq` of zero reads the whole journal.
///
/// # Errors
/// Returns SQLite's message; an unknown kind or undecodable data is an
/// error too, because replay must never silently skip history.
pub fn read_from(
    storage: &mut crate::storage::SqliteStorage,
    from_seq: i64,
) -> Result<Vec<Entry>, String> {
    let connection = storage.platform();
    let mut statement = connection
        .prepare(
            "SELECT seq, at, kind, data, format FROM __actias_wf_journal
             WHERE seq >= ? ORDER BY seq",
        )
        .map_err(|e| e.to_string())?;

    let rows = statement
        .query_map(rusqlite::params![from_seq], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut entries = Vec::new();
    for row in rows {
        let (seq, at, kind, data, format) = row.map_err(|e| e.to_string())?;
        let kind = EntryKind::parse(&kind)
            .ok_or_else(|| format!("journal entry {seq} has unknown kind '{kind}'"))?;
        let data = serde_json::from_str(&data)
            .map_err(|e| format!("journal entry {seq} does not decode: {e}"))?;
        entries.push(Entry {
            seq,
            at,
            kind,
            data,
            format,
        });
    }
    Ok(entries)
}

/// The newest entry, if any: what `status()` reads and what the console
/// lists instances by. Never a visibility store, just the head.
///
/// # Errors
/// Returns SQLite's message.
pub fn head(storage: &mut crate::storage::SqliteStorage) -> Result<Option<Entry>, String> {
    let mut entries = read_from_limit(storage, 1)?;
    Ok(entries.pop())
}

/// The newest `limit` entries, newest first; the head helper rides it.
fn read_from_limit(
    storage: &mut crate::storage::SqliteStorage,
    limit: i64,
) -> Result<Vec<Entry>, String> {
    let connection = storage.platform();
    let mut statement = connection
        .prepare(
            "SELECT seq, at, kind, data, format FROM __actias_wf_journal
             ORDER BY seq DESC LIMIT ?",
        )
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map(rusqlite::params![limit], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut entries = Vec::new();
    for row in rows {
        let (seq, at, kind, data, format) = row.map_err(|e| e.to_string())?;
        let kind = EntryKind::parse(&kind)
            .ok_or_else(|| format!("journal entry {seq} has unknown kind '{kind}'"))?;
        let data = serde_json::from_str(&data)
            .map_err(|e| format!("journal entry {seq} does not decode: {e}"))?;
        entries.push(Entry {
            seq,
            at,
            kind,
            data,
            format,
        });
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::SqliteStorage;

    fn open(dir: &tempfile::TempDir) -> SqliteStorage {
        let mut storage = SqliteStorage::open(&dir.path().join("wf.db")).expect("opens");
        ensure_schema(&mut storage).expect("schema");
        storage
    }

    #[test]
    fn appends_read_back_in_order_across_reopen() {
        let dir = tempfile::tempdir().expect("tempdir");
        {
            let mut storage = open(&dir);
            append(
                &mut storage,
                EntryKind::Started,
                &serde_json::json!({ "revision": "rev-1", "seed": 42 }),
            )
            .expect("appends");
            append(
                &mut storage,
                EntryKind::Intent,
                &serde_json::json!({ "step": "charge-card" }),
            )
            .expect("appends");
        }

        // A new open over the same file: the journal IS the durability.
        let mut storage = open(&dir);
        let seq = append(
            &mut storage,
            EntryKind::Result,
            &serde_json::json!({ "step": "charge-card", "value": { "ok": true } }),
        )
        .expect("appends");
        assert_eq!(seq, 3, "sequence continues across reopen");

        let entries = read_from(&mut storage, 0).expect("reads");
        assert_eq!(
            entries.iter().map(|e| e.kind).collect::<Vec<_>>(),
            vec![EntryKind::Started, EntryKind::Intent, EntryKind::Result],
        );
        assert_eq!(entries[0].data["seed"], 42);
        assert!(entries.iter().all(|e| e.format == ENTRY_FORMAT));

        // The cursor read: replay resumes past what it already consumed.
        let tail = read_from(&mut storage, 2).expect("reads");
        assert_eq!(tail.len(), 2);
        assert_eq!(tail[0].kind, EntryKind::Intent);
    }

    #[test]
    fn the_head_is_the_newest_entry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut storage = open(&dir);
        assert_eq!(head(&mut storage).expect("reads"), None);

        append(&mut storage, EntryKind::Started, &serde_json::json!({})).expect("appends");
        append(
            &mut storage,
            EntryKind::Completed,
            &serde_json::json!({ "value": "fulfilled" }),
        )
        .expect("appends");

        let newest = head(&mut storage).expect("reads").expect("has entries");
        assert_eq!(newest.kind, EntryKind::Completed);
        assert_eq!(newest.seq, 2);
    }

    #[test]
    fn ensure_schema_is_idempotent_and_versioned() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut storage = open(&dir);
        // A second ensure over a current file is a no-op, not an error.
        ensure_schema(&mut storage).expect("idempotent");
        assert_eq!(storage.schema_version().expect("reads"), SCHEMA_VERSION);
    }
}
