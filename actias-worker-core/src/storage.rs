//! Durable per-object storage: one SQLite file per object identity,
//! owned by the object's pinned task and never crossing threads. This is
//! deliberately a concrete type, not a trait: the surface is SQL, so only
//! a SQL engine could ever implement it, and the plausible futures (a
//! restored snapshot, the `actias test` fake, WAL shipping) are all still
//! this type - opened on a different file, in memory, or with a shipper
//! watching the WAL beside it.

use std::path::Path;

/// The storage as vm app data: one cell per pinned vm, borrowed only
/// inside a method call, which is single-threaded by construction.
pub struct StorageCell(pub std::cell::RefCell<SqliteStorage>);

/// SQLite on a local file, durability by fsync: `synchronous=FULL` under
/// WAL, so every committed write survives a crash without an explicit
/// flush step. Checkpoint trims the WAL so files stay small.
pub struct SqliteStorage {
    connection: rusqlite::Connection,
}

impl SqliteStorage {
    /// Opens (or creates) the object's file.
    ///
    /// # Errors
    /// Returns text when the file cannot be opened or configured.
    pub fn open(path: &Path) -> Result<Self, String> {
        let connection = rusqlite::Connection::open(path).map_err(|e| e.to_string())?;

        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| e.to_string())?;
        connection
            .pragma_update(None, "synchronous", "FULL")
            .map_err(|e| e.to_string())?;

        Ok(Self { connection })
    }

    /// An in-memory database, for tests and local fakes.
    ///
    /// # Errors
    /// Returns text when SQLite cannot create it.
    pub fn in_memory() -> Result<Self, String> {
        Ok(Self {
            connection: rusqlite::Connection::open_in_memory().map_err(|e| e.to_string())?,
        })
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

    /// The persisted alarm, if one is set: (due unix ms, class, own key).
    ///
    /// # Errors
    /// Returns SQLite's message.
    pub fn load_alarm(&mut self) -> Result<Option<(i64, String, String)>, String> {
        self.ensure_meta()?;
        let mut statement = self
            .connection
            .prepare("SELECT due_ms, class, own_key FROM __actias_alarm")
            .map_err(|e| e.to_string())?;
        let mut rows = statement.query([]).map_err(|e| e.to_string())?;

        match rows.next().map_err(|e| e.to_string())? {
            Some(row) => Ok(Some((
                row.get(0).map_err(|e| e.to_string())?,
                row.get(1).map_err(|e| e.to_string())?,
                row.get(2).map_err(|e| e.to_string())?,
            ))),
            None => Ok(None),
        }
    }

    /// Persists the one alarm (an object has at most one; setting replaces).
    ///
    /// # Errors
    /// Returns SQLite's message.
    pub fn save_alarm(&mut self, due_ms: i64, class: &str, own_key: &str) -> Result<(), String> {
        self.ensure_meta()?;
        self.connection
            .execute("DELETE FROM __actias_alarm", [])
            .map_err(|e| e.to_string())?;
        self.connection
            .execute(
                "INSERT INTO __actias_alarm (due_ms, class, own_key) VALUES (?, ?, ?)",
                rusqlite::params![due_ms, class, own_key],
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

    /// The reserved platform table; `__` prefixes are refused to scripts.
    fn ensure_meta(&mut self) -> Result<(), String> {
        self.connection
            .execute(
                "CREATE TABLE IF NOT EXISTS __actias_alarm                  (due_ms INTEGER NOT NULL, class TEXT NOT NULL, own_key TEXT NOT NULL)",
                [],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
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

        self.connection
            .execute(sql, rusqlite::params_from_iter(bound))
            .map(|rows| rows as u64)
            .map_err(|e| e.to_string())
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

        let mut statement = self.connection.prepare(sql).map_err(|e| e.to_string())?;
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
    /// between calls (writes are already durable under synchronous=FULL).
    ///
    /// # Errors
    /// Returns SQLite's own message.
    pub fn checkpoint(&mut self) -> Result<(), String> {
        // Writes are already durable (synchronous=FULL); this only keeps
        // the WAL from growing between calls.
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
    fn an_unbindable_parameter_is_refused() {
        let mut storage = SqliteStorage::in_memory().expect("opens");

        let error = storage
            .exec("SELECT ?", &[serde_json::json!({ "nested": true })])
            .expect_err("tables cannot bind");
        assert!(error.contains("not bindable"), "{error}");
    }
}
