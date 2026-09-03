//! Durable per-object storage: one SQLite file per object identity,
//! owned by the object's pinned task and never crossing threads. This is
//! deliberately a concrete type, not a trait: the surface is SQL, so only
//! a SQL engine could ever implement it, and the plausible futures (a
//! restored snapshot, the `actias test` fake, WAL shipping) are all still
//! this type: opened on a different file, in memory, or with a shipper
//! watching the WAL beside it.

use std::path::Path;

/// SQLite on a local file in WAL mode with `synchronous=NORMAL`: a
/// commit reaches the OS, not the disk. The file is a leased cache of
/// state whose commit point is elsewhere (the shipped manifest, the
/// replica quorum), so the per-commit fsync bought nothing any promise
/// depends on and cost every write a disk round trip. The WAL is still
/// fsynced at each checkpoint, and the shipper reads committed frames
/// from the OS's copy. Checkpoint trims the WAL so files stay small.
pub struct SqliteStorage {
    connection: rusqlite::Connection,
    /// Whether script-issued statements and the platform's own
    /// key-value verbs currently refuse to write. Set only for the
    /// window [`SqliteStorage::read_only`] scopes.
    read_only: bool,
}

/// How long a connection waits for a lock another one holds. Contention
/// here is momentary by construction, so waiting resolves it.
const BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

impl SqliteStorage {
    /// Opens (or creates) the object's file.
    ///
    /// # Errors
    /// Returns text when the file cannot be opened or configured.
    pub fn open(path: &Path) -> Result<Self, String> {
        let connection = rusqlite::Connection::open(path).map_err(|e| e.to_string())?;

        // The shipper opens its own connection to this file to snapshot
        // and checkpoint while this one commits. Sqlite fails such a
        // collision instantly by default, which loses writes that only
        // needed to wait a moment.
        connection
            .busy_timeout(BUSY_TIMEOUT)
            .map_err(|e| e.to_string())?;
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| e.to_string())?;
        connection
            .pragma_update(None, "synchronous", "NORMAL")
            .map_err(|e| e.to_string())?;
        // The WAL folds when the shipper decides, never behind our back:
        // an implicit checkpoint mid-flight would move bytes the frame
        // reader is about to ship.
        connection
            .pragma_update(None, "wal_autocheckpoint", 0)
            .map_err(|e| e.to_string())?;

        Ok(Self {
            connection,
            read_only: false,
        })
    }

    /// A consistent snapshot of the database into `dest`, taken through
    /// sqlite itself, so it is safe beside a live writer; copying the
    /// file's bytes is not. The copy is compacted and carries every
    /// committed write, checkpointed or not.
    ///
    /// # Errors
    /// Returns SQLite's message.
    pub fn snapshot_to(&mut self, dest: &Path) -> Result<(), String> {
        let _ = std::fs::remove_file(dest);
        let quoted = dest.to_string_lossy().replace('\'', "''");
        self.connection
            .execute_batch(&format!("VACUUM INTO '{quoted}'"))
            .map_err(|e| e.to_string())
    }

    /// Folds what it can of the WAL into the main file without waiting
    /// on anyone; the shipper calls it to bound log growth, and a busy
    /// answer is fine because the next flight retries.
    ///
    /// # Errors
    /// Returns SQLite's message. A busy database is one of them.
    pub fn checkpoint_passive(&mut self) -> Result<(), String> {
        self.connection
            .pragma_update(None, "wal_checkpoint", "PASSIVE")
            .map_err(|e| e.to_string())
    }

    /// Opens the file read-only, for reads that bypass the owner's
    /// mailbox: they see every committed write and nothing in flight,
    /// which is the bounded staleness the bypass trades on.
    ///
    /// # Errors
    /// Returns text when the file cannot be opened.
    pub fn open_read_only(path: &Path) -> Result<Self, String> {
        let connection =
            rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|e| e.to_string())?;
        connection
            .busy_timeout(BUSY_TIMEOUT)
            .map_err(|e| e.to_string())?;

        Ok(Self {
            connection,
            read_only: true,
        })
    }

    /// An in-memory database, for tests and local fakes.
    ///
    /// # Errors
    /// Returns text when SQLite cannot create it.
    pub fn in_memory() -> Result<Self, String> {
        Ok(Self {
            connection: rusqlite::Connection::open_in_memory().map_err(|e| e.to_string())?,
            read_only: false,
        })
    }

    /// Refuses writes until cleared: script sql is authorized
    /// read-only and the platform's key-value verbs refuse too, so a
    /// directory evaluation cannot change the state it describes.
    ///
    /// A setter rather than a scoped closure on purpose. The guest
    /// code this scopes reaches storage through the same lock the
    /// scope would hold, so a closure spanning the call would
    /// deadlock on the first `state.sql` read. The caller sets, calls,
    /// and clears unconditionally.
    pub fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
    }

    /// Refuses a platform write while the storage is read-only,
    /// naming what was attempted. The authorizer covers script sql;
    /// this covers the verbs that drive the connection directly.
    ///
    /// # Errors
    /// Returns the refusal when [`SqliteStorage::read_only`] scopes
    /// the caller.
    pub(crate) fn refuse_if_read_only(&self, what: &str) -> Result<(), String> {
        if self.read_only {
            return Err(format!(
                "{what} is not allowed here: this code runs read-only."
            ));
        }
        Ok(())
    }

    /// The guard script-issued sql runs under, which the read-only
    /// window swaps.
    fn guard(&self) -> fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization {
        if self.read_only {
            read_only_authorizer
        } else {
            script_authorizer
        }
    }
}

impl SqliteStorage {
    /// Whether this file has never been initialized: `init` runs exactly
    /// once per object, and the file itself is the record of that.
    ///
    /// # Errors
    /// Returns SQLite's message.
    pub fn is_fresh(&mut self) -> Result<bool, String> {
        let version: i64 = self
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(|e| e.to_string())?;
        Ok(version == 0)
    }

    /// Marks `init` as having completed; runs after a successful init so a
    /// failed one retries on the next call.
    ///
    /// # Errors
    /// Returns SQLite's message.
    pub fn mark_initialized(&mut self) -> Result<(), String> {
        self.connection
            .pragma_update(None, "user_version", 1)
            .map_err(|e| e.to_string())
    }

    /// Caps the file at `bytes`: SQLite's own page limit, so a write past
    /// the quota fails at the statement ("database or disk is full") while
    /// everything already stored stays readable.
    ///
    /// # Errors
    /// Returns SQLite's message.
    pub fn set_size_limit(&mut self, bytes: u64) -> Result<(), String> {
        let page_size: i64 = self
            .connection
            .pragma_query_value(None, "page_size", |row| row.get(0))
            .map_err(|e| e.to_string())?;
        let pages = (bytes as i64 / page_size.max(1)).max(1);

        self.connection
            .pragma_update(None, "max_page_count", pages)
            .map_err(|e| e.to_string())
    }

    /// Runs one migration file under the script guard: user sql, so the
    /// same rules as any statement, but as a whole script since migration
    /// files hold several statements.
    ///
    /// # Errors
    /// Returns SQLite's message.
    pub fn exec_script(&mut self, sql: &str) -> Result<(), String> {
        self.connection.authorizer(Some(self.guard()));
        let result = self
            .connection
            .execute_batch(sql)
            .map_err(|e| e.to_string());
        self.connection.authorizer(
            None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>,
        );
        result
    }

    /// Migration names already applied to this database, sorted.
    ///
    /// # Errors
    /// Returns SQLite's message.
    pub fn applied_migrations(&mut self) -> Result<Vec<String>, String> {
        self.connection
            .execute(
                "CREATE TABLE IF NOT EXISTS __actias_migrations (name TEXT PRIMARY KEY)",
                [],
            )
            .map_err(|e| e.to_string())?;

        let mut statement = self
            .connection
            .prepare("SELECT name FROM __actias_migrations ORDER BY name")
            .map_err(|e| e.to_string())?;
        let names = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(names)
    }

    /// Records one migration as applied; rides the call's transaction, so
    /// a failed migration records nothing.
    ///
    /// # Errors
    /// Returns SQLite's message.
    pub fn record_migration(&mut self, name: &str) -> Result<(), String> {
        self.connection
            .execute(
                "INSERT INTO __actias_migrations (name) VALUES (?)",
                rusqlite::params![name],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Total rows changed over this connection's lifetime; advancing
    /// between two moments is what "this call wrote" means.
    ///
    /// # Errors
    /// Returns SQLite's message.
    pub fn total_changes(&mut self) -> Result<i64, String> {
        self.connection
            .query_row("SELECT total_changes()", [], |row| row.get(0))
            .map_err(|e| e.to_string())
    }

    /// Opens the transaction one dispatched call runs inside; the platform
    /// owns transaction boundaries, scripts never issue their own.
    ///
    /// # Errors
    /// Returns SQLite's message.
    pub fn begin(&mut self) -> Result<(), String> {
        // IMMEDIATE, not DEFERRED: the object is the file's only writer,
        // so taking the write lock up front costs nothing, and it is what
        // keeps the shipper's TRUNCATE checkpoint from resetting the WAL
        // between a call's first read and its first write. A deferred
        // transaction that read before the reset cannot upgrade and fails
        // at once with SQLITE_BUSY, past the busy handler.
        self.connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| e.to_string())
    }

    /// Commits the call's transaction.
    ///
    /// # Errors
    /// Returns SQLite's message.
    pub fn commit(&mut self) -> Result<(), String> {
        self.connection
            .execute_batch("COMMIT")
            .map_err(|e| e.to_string())
    }

    /// Rolls the call's transaction back; a failed method persists nothing.
    ///
    /// # Errors
    /// Returns SQLite's message.
    pub fn rollback(&mut self) -> Result<(), String> {
        self.connection
            .execute_batch("ROLLBACK")
            .map_err(|e| e.to_string())
    }

    /// The file's schema version (SQLite's user_version cell); 0 means
    /// fresh, and each platform class owns what its numbers mean.
    ///
    /// # Errors
    /// Returns SQLite's message.
    pub fn schema_version(&mut self) -> Result<i64, String> {
        self.connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(|e| e.to_string())
    }

    /// Stamps the file's schema version.
    ///
    /// # Errors
    /// Returns SQLite's message.
    pub fn set_schema_version(&mut self, version: i64) -> Result<(), String> {
        self.connection
            .pragma_update(None, "user_version", version)
            .map_err(|e| e.to_string())
    }

    /// Whether the file holds a table of that name; how read paths decide
    /// what a file that predates a schema (a fresh object, an old
    /// snapshot, a replica) contains, instead of classifying errors.
    ///
    /// # Errors
    /// Returns SQLite's message.
    pub fn table_exists(&mut self, name: &str) -> Result<bool, String> {
        self.connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?)",
                [name],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())
    }

    /// Like [`Self::load_alarm`] but safe on read-only connections: a file
    /// that never held an alarm simply has no table, which reads as none.
    ///
    /// # Errors
    /// Returns SQLite's message.
    pub fn peek_alarm(&mut self) -> Result<Option<(i64, String, String, String)>, String> {
        if !self.table_exists(ALARM_TABLE)? {
            return Ok(None);
        }
        self.load_alarm_row()
    }

    /// The persisted alarm, if one is set: (due unix ms, class, instance
    /// name, own key).
    ///
    /// # Errors
    /// Returns SQLite's message.
    pub fn load_alarm(&mut self) -> Result<Option<(i64, String, String, String)>, String> {
        self.ensure_meta()?;
        self.load_alarm_row()
    }

    /// The alarm row itself, assuming the table exists.
    fn load_alarm_row(&mut self) -> Result<Option<(i64, String, String, String)>, String> {
        let mut statement = self
            .connection
            .prepare("SELECT due_ms, class, name, own_key FROM __actias_alarm")
            .map_err(|e| e.to_string())?;
        let mut rows = statement.query([]).map_err(|e| e.to_string())?;

        match rows.next().map_err(|e| e.to_string())? {
            Some(row) => Ok(Some((
                row.get(0).map_err(|e| e.to_string())?,
                row.get(1).map_err(|e| e.to_string())?,
                row.get(2).map_err(|e| e.to_string())?,
                row.get(3).map_err(|e| e.to_string())?,
            ))),
            None => Ok(None),
        }
    }

    /// Persists the one alarm (an object has at most one; setting replaces).
    ///
    /// # Errors
    /// Returns SQLite's message.
    pub fn save_alarm(
        &mut self,
        due_ms: i64,
        class: &str,
        name: &str,
        own_key: &str,
    ) -> Result<(), String> {
        self.ensure_meta()?;
        self.connection
            .execute("DELETE FROM __actias_alarm", [])
            .map_err(|e| e.to_string())?;
        self.connection
            .execute(
                "INSERT INTO __actias_alarm (due_ms, class, name, own_key) VALUES (?, ?, ?, ?)",
                rusqlite::params![due_ms, class, name, own_key],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Drops the persisted alarm; called the moment it fires, so a handler
    /// that sets a new one is not clobbered afterwards.
    ///
    /// # Errors
    /// Returns SQLite's message.
    pub fn clear_alarm(&mut self) -> Result<(), String> {
        self.ensure_meta()?;
        self.connection
            .execute("DELETE FROM __actias_alarm", [])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// The bare connection, for platform-owned statements. The script
    /// guard exists only around script-issued SQL; platform modules drive
    /// the connection directly, the way the alarm and migration helpers
    /// here do.
    pub(crate) fn platform(&mut self) -> &mut rusqlite::Connection {
        &mut self.connection
    }

    /// The reserved platform table; `__` prefixes are refused to scripts.
    fn ensure_meta(&mut self) -> Result<(), String> {
        self.connection
            .execute(
                "CREATE TABLE IF NOT EXISTS __actias_alarm                  (due_ms INTEGER NOT NULL, class TEXT NOT NULL, name TEXT NOT NULL, own_key TEXT NOT NULL)",
                [],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// The alarm table's name, for existence probes; must match the DDL in
/// [`SqliteStorage::ensure_meta`].
const ALARM_TABLE: &str = "__actias_alarm";

/// What script-issued SQL may do while the storage is read-only: the
/// script guard, plus a refusal of everything that writes. Directory
/// evaluation runs under this, which is what lets the same function be
/// re-derived from a restored copy on any node, with no lease and no
/// mailbox: a function that cannot write cannot need one.
fn read_only_authorizer(
    context: rusqlite::hooks::AuthContext<'_>,
) -> rusqlite::hooks::Authorization {
    use rusqlite::hooks::{AuthAction, Authorization};

    match &context.action {
        AuthAction::Insert { .. }
        | AuthAction::Update { .. }
        | AuthAction::Delete { .. }
        | AuthAction::CreateTable { .. }
        | AuthAction::CreateIndex { .. }
        | AuthAction::CreateTrigger { .. }
        | AuthAction::DropTable { .. }
        | AuthAction::DropIndex { .. }
        | AuthAction::DropTrigger { .. }
        | AuthAction::AlterTable { .. } => Authorization::Deny,
        _ => script_authorizer(context),
    }
}

/// What script-issued SQL may do. Platform paths (the dispatch
/// transaction, meta tables, pragmas at open) run without this guard;
/// everything a script writes runs under it.
fn script_authorizer(context: rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization {
    use rusqlite::hooks::{AuthAction, Authorization};

    let table = match &context.action {
        // Other files and other databases are not this object's world.
        AuthAction::Attach { .. } | AuthAction::Detach { .. } => return Authorization::Deny,
        // The platform owns transaction boundaries; a script BEGIN would
        // corrupt the all-or-nothing guarantee of its own call.
        AuthAction::Transaction { .. } => return Authorization::Deny,
        // Pragmas can flip durability off or reset the init marker.
        AuthAction::Pragma { .. } => return Authorization::Deny,

        AuthAction::Read { table_name, .. } => table_name,
        AuthAction::Insert { table_name } => table_name,
        AuthAction::Update { table_name, .. } => table_name,
        AuthAction::Delete { table_name } => table_name,
        AuthAction::CreateTable { table_name } => table_name,
        AuthAction::CreateIndex { table_name, .. } => table_name,
        AuthAction::CreateTrigger { table_name, .. } => table_name,
        AuthAction::DropTable { table_name } => table_name,
        AuthAction::DropIndex { table_name, .. } => table_name,
        AuthAction::DropTrigger { table_name, .. } => table_name,
        AuthAction::AlterTable { table_name, .. } => table_name,

        _ => return Authorization::Allow,
    };

    // Reserved rows (alarms, and migrations later) belong to the platform.
    if table.starts_with("__actias_") {
        return Authorization::Deny;
    }

    Authorization::Allow
}

/// One json parameter as something SQLite can bind.
fn bind(value: &serde_json::Value) -> Result<rusqlite::types::Value, String> {
    use rusqlite::types::Value;

    Ok(match value {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Integer(i64::from(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Integer(i)
            } else {
                Value::Real(n.as_f64().unwrap_or(f64::NAN))
            }
        }
        serde_json::Value::String(s) => Value::Text(s.clone()),
        other => {
            return Err(format!(
                "Parameter {other} is not bindable; pass strings, numbers, booleans or nil."
            ));
        }
    })
}

/// One SQLite value back as json.
fn unbind(value: rusqlite::types::ValueRef<'_>) -> serde_json::Value {
    use rusqlite::types::ValueRef;

    match value {
        ValueRef::Null => serde_json::Value::Null,
        ValueRef::Integer(i) => serde_json::Value::from(i),
        ValueRef::Real(f) => serde_json::Value::from(f),
        ValueRef::Text(t) => serde_json::Value::from(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => serde_json::Value::from(
            b.iter()
                .map(|byte| serde_json::Value::from(*byte))
                .collect::<Vec<_>>(),
        ),
    }
}

impl SqliteStorage {
    /// Runs a statement; returns affected rows. `&mut self` everywhere
    /// because exactly one call runs at a time by construction.
    ///
    /// # Errors
    /// Returns SQLite's own message; it is the script author's SQL.
    pub fn exec(&mut self, sql: &str, params: &[serde_json::Value]) -> Result<u64, String> {
        let bound: Vec<rusqlite::types::Value> =
            params.iter().map(bind).collect::<Result<_, _>>()?;

        self.connection.authorizer(Some(self.guard()));
        let result = self
            .connection
            .execute(sql, rusqlite::params_from_iter(bound))
            .map(|rows| rows as u64)
            .map_err(|e| e.to_string());
        self.connection.authorizer(
            None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>,
        );

        result
    }

    /// Runs a query; every row becomes a name-to-value object.
    ///
    /// # Errors
    /// Returns SQLite's own message; it is the script author's SQL.
    pub fn query(
        &mut self,
        sql: &str,
        params: &[serde_json::Value],
    ) -> Result<Vec<serde_json::Value>, String> {
        let bound: Vec<rusqlite::types::Value> =
            params.iter().map(bind).collect::<Result<_, _>>()?;

        self.connection.authorizer(Some(self.guard()));
        let prepared = self.connection.prepare(sql);
        self.connection.authorizer(
            None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>,
        );
        let mut statement = prepared.map_err(|e| e.to_string())?;
        let names: Vec<String> = statement
            .column_names()
            .into_iter()
            .map(str::to_owned)
            .collect();

        let mut rows = statement
            .query(rusqlite::params_from_iter(bound))
            .map_err(|e| e.to_string())?;

        let mut result = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let mut object = serde_json::Map::new();
            for (index, name) in names.iter().enumerate() {
                let value = row.get_ref(index).map_err(|e| e.to_string())?;
                object.insert(name.clone(), unbind(value));
            }
            result.push(serde_json::Value::Object(object));
        }

        Ok(result)
    }

    /// Called after each handler completes; keeps the WAL from growing
    /// between calls; the checkpoint is also the fsync under
    /// synchronous=NORMAL.
    ///
    /// # Errors
    /// Returns SQLite's own message.
    pub fn checkpoint(&mut self) -> Result<(), String> {
        self.connection
            .pragma_update(None, "wal_checkpoint", "TRUNCATE")
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_round_trip_with_their_types() {
        let mut storage = SqliteStorage::in_memory().expect("opens");

        storage
            .exec(
                "CREATE TABLE t (n INTEGER, f REAL, s TEXT, missing TEXT)",
                &[],
            )
            .expect("creates");
        storage
            .exec(
                "INSERT INTO t VALUES (?, ?, ?, ?)",
                &[
                    serde_json::json!(7),
                    serde_json::json!(1.5),
                    serde_json::json!("hello"),
                    serde_json::Value::Null,
                ],
            )
            .expect("inserts");

        let rows = storage.query("SELECT * FROM t", &[]).expect("queries");
        assert_eq!(
            rows,
            vec![serde_json::json!({ "n": 7, "f": 1.5, "s": "hello", "missing": null })]
        );
    }

    #[test]
    fn a_file_reopens_with_its_rows() {
        // The restart story at unit scale: a fresh connection over the same
        // file sees everything a committed write left.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("object.db");

        let mut storage = SqliteStorage::open(&path).expect("opens");
        storage
            .exec("CREATE TABLE hits (at INTEGER)", &[])
            .expect("creates");
        storage
            .exec("INSERT INTO hits VALUES (1)", &[])
            .expect("inserts");
        storage.checkpoint().expect("checkpoints");
        drop(storage);

        let mut reopened = SqliteStorage::open(&path).expect("reopens");
        let rows = reopened
            .query("SELECT COUNT(*) AS n FROM hits", &[])
            .expect("queries");
        assert_eq!(rows, vec![serde_json::json!({ "n": 1 })]);
    }

    #[test]
    fn script_sql_cannot_escape_its_world() {
        let mut storage = SqliteStorage::in_memory().expect("opens");
        storage
            .exec("CREATE TABLE t (n INTEGER)", &[])
            .expect("plain ddl is allowed");

        for refused in [
            "ATTACH DATABASE '/etc/passwd' AS other",
            "BEGIN",
            "COMMIT",
            "PRAGMA user_version = 0",
            "PRAGMA journal_mode = OFF",
            "SELECT * FROM __actias_alarm",
            "DROP TABLE __actias_alarm",
        ] {
            // The reserved table must exist for the reads to even parse.
            storage.ensure_meta().expect("meta ensured");
            let error = storage
                .exec(refused, &[])
                .expect_err(&format!("{refused:?} must be refused"));
            assert!(
                error.contains("authoriz") || error.contains("prohibited"),
                "{refused:?}: {error}"
            );
        }

        // The guard is scoped to script sql: the platform still owns its
        // transactions and meta afterwards.
        storage.begin().expect("platform begin still works");
        storage.rollback().expect("platform rollback still works");
        storage.load_alarm().expect("platform meta still reachable");
    }

    #[test]
    fn a_database_over_quota_refuses_writes_but_still_reads() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut storage = SqliteStorage::open(&dir.path().join("small.db")).expect("opens");
        // A handful of pages: room for the schema, not for the flood.
        storage.set_size_limit(16 * 4096).expect("limit sets");

        storage
            .exec("CREATE TABLE t (blob TEXT)", &[])
            .expect("creates");

        let filler = "x".repeat(4096);
        let mut refused = None;
        for _ in 0..64 {
            if let Err(error) = storage.exec(
                "INSERT INTO t VALUES (?)",
                &[serde_json::json!(filler.clone())],
            ) {
                refused = Some(error);
                break;
            }
        }

        let refused = refused.expect("the quota must eventually refuse");
        assert!(refused.contains("full"), "{refused}");

        // Stored rows stay readable past the quota.
        storage
            .query("SELECT COUNT(*) AS n FROM t", &[])
            .expect("reads still work");
    }

    #[test]
    fn a_bypassed_read_sees_committed_rows_and_never_blocks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("owner.db");

        let mut owner = SqliteStorage::open(&path).expect("opens");
        owner.exec("CREATE TABLE t (n INTEGER)", &[]).expect("ddl");
        owner
            .exec("INSERT INTO t VALUES (1)", &[])
            .expect("committed row");

        // The owner holds an open transaction with an uncommitted write,
        // exactly the state a long-running method would pin.
        owner.begin().expect("begins");
        owner
            .exec("INSERT INTO t VALUES (2)", &[])
            .expect("in-flight row");

        let mut reader = SqliteStorage::open_read_only(&path).expect("read-only opens");
        let rows = reader
            .query("SELECT COUNT(*) AS n FROM t", &[])
            .expect("the read returns without waiting");
        assert_eq!(rows, vec![serde_json::json!({ "n": 1 })]);

        owner.rollback().expect("rolls back");
    }

    #[test]
    fn an_unbindable_parameter_is_refused() {
        let mut storage = SqliteStorage::in_memory().expect("opens");

        let error = storage
            .exec("SELECT ?", &[serde_json::json!({ "nested": true })])
            .expect_err("tables cannot bind");
        assert!(error.contains("not bindable"), "{error}");
    }
}
