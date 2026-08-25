//! The `__database` platform class: the sql product face, an ordinary
//! object whose methods are the storage surface.
//!
//! [`Database`] is the typed API: rust callers (the dispatch codec below,
//! and any future api read path or test) open a handle and call real
//! methods. The statements are user SQL, so unlike the other platform
//! classes every one of them runs through the script-guarded
//! [`SqliteStorage`] surface, never the bare connection.

use crate::extensions::objects::DATABASE_CLASS;
use crate::objects::ObjectHome;
use crate::storage::SqliteStorage;

/// A typed handle to one database instance's operations.
pub struct Database<'a> {
    home: &'a ObjectHome,
}

impl<'a> Database<'a> {
    /// Opens the instance. A declared `migrations_dir` is applied first
    /// when this is the vm's first touch: the tracking rows ride the
    /// call's transaction, so a failed migration applies nothing,
    /// records nothing, and retries on the next touch. Without one the
    /// database is manual: the script owns its own schema.
    ///
    /// # Errors
    /// Returns the failed migration's user-safe message.
    pub fn open(home: &'a ObjectHome, migrations_dir: Option<&str>) -> Result<Self, String> {
        if let Some(dir) = migrations_dir {
            Self::apply_declared_migrations(home, dir)?;
        }
        Ok(Self { home })
    }

    /// Runs one statement; the affected row count is the result.
    ///
    /// # Errors
    /// Returns a refused statement's or SQLite's user-safe message.
    pub fn exec(&self, sql: &str, params: &[serde_json::Value]) -> Result<u64, String> {
        self.home.with_storage(|storage| storage.exec(sql, params))
    }

    /// Runs one query; rows come back as string-keyed json objects.
    ///
    /// # Errors
    /// Returns a refused statement's or SQLite's user-safe message.
    pub fn query(
        &self,
        sql: &str,
        params: &[serde_json::Value],
    ) -> Result<Vec<serde_json::Value>, String> {
        self.home.with_storage(|storage| storage.query(sql, params))
    }

    /// [`Self::query`] returning only the first row, if any.
    ///
    /// # Errors
    /// Returns a refused statement's or SQLite's user-safe message.
    pub fn query_one(
        &self,
        sql: &str,
        params: &[serde_json::Value],
    ) -> Result<Option<serde_json::Value>, String> {
        Ok(self.query(sql, params)?.into_iter().next())
    }

    /// Runs statements in order, returning each one's affected count. A
    /// batch is nothing special: one call is one transaction already, so
    /// the atomicity comes from the dispatch guard.
    ///
    /// # Errors
    /// Returns the first failing statement's user-safe message.
    pub fn batch(
        &self,
        statements: &[(String, Vec<serde_json::Value>)],
    ) -> Result<Vec<u64>, String> {
        statements
            .iter()
            .map(|(sql, params)| self.exec(sql, params))
            .collect()
    }

    /// Applies the `.sql` files a declaration named to this instance's
    /// file, once per vm, before its first call proceeds. Pending files
    /// run in the touching call's transaction, so a failure applies and
    /// records nothing.
    ///
    /// # Errors
    /// Returns the failed migration's user-safe message.
    pub fn apply_declared_migrations(home: &ObjectHome, dir: &str) -> Result<(), String> {
        if !home.migrations_unchecked() {
            return Ok(());
        }
        let Some(revision) = home.revision() else {
            return Err("Runtime has no revision loaded.".to_owned());
        };
        Database { home }.run_migrations(revision.migrations_in(dir))?;
        home.mark_migrations_checked();
        Ok(())
    }

    /// Applies the given migration files, skipping any already recorded.
    fn run_migrations(&self, migrations: Vec<(String, String)>) -> Result<(), String> {
        if migrations.is_empty() {
            return Ok(());
        }

        self.home.with_storage(|storage: &mut SqliteStorage| {
            let applied = storage.applied_migrations()?;
            for (name, sql) in migrations {
                if applied.contains(&name) {
                    continue;
                }
                storage
                    .exec_script(&sql)
                    .map_err(|error| format!("Migration {name} failed: {error}"))?;
                storage.record_migration(&name)?;
            }
            Ok(())
        })
    }
}

/// The wire codec: maps one dispatched method name and its json arguments
/// onto [`Database`], and the typed result back to json. Method names
/// arrive as strings because that is what a Lua handle sends.
pub(crate) fn dispatch(
    context: &super::PlatformContext<'_>,
    call: &super::Call,
) -> Result<serde_json::Value, String> {
    let database = Database::open(context.home, context.migrations_dir.as_deref())?;

    match call.method.as_str() {
        "exec" => {
            let (sql, params) = statement(&call.args)?;
            database.exec(&sql, &params)?;
            Ok(serde_json::Value::Bool(true))
        }
        "query" | "read" => {
            let (sql, params) = statement(&call.args)?;
            Ok(serde_json::Value::Array(database.query(&sql, &params)?))
        }
        "query_one" | "read_one" => {
            let (sql, params) = statement(&call.args)?;
            Ok(database
                .query_one(&sql, &params)?
                .unwrap_or(serde_json::Value::Null))
        }
        "batch" => {
            let entries = call
                .args
                .first()
                .and_then(|value| value.as_array())
                .ok_or_else(|| "batch takes a list of statements.".to_owned())?;
            let statements = entries
                .iter()
                .map(|entry| {
                    entry
                        .as_array()
                        .ok_or_else(|| "Each batch entry is { sql, params }.".to_owned())
                        .and_then(|parts| statement(parts))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(serde_json::json!(database.batch(&statements)?))
        }
        other => Err(format!(
            "Object class '{DATABASE_CLASS}' has no method '{other}'."
        )),
    }
}

/// One statement's text and positional parameters, as the handle sends
/// them: `[sql]` or `[sql, [params...]]`.
fn statement(args: &[serde_json::Value]) -> Result<(String, Vec<serde_json::Value>), String> {
    let text = args
        .first()
        .and_then(|value| value.as_str())
        .ok_or_else(|| "The statement text must be a string.".to_owned())?;
    let params = args
        .get(1)
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    Ok((text.to_owned(), params))
}

/// One column's shape, straight from SQLite's own table metadata.
#[derive(serde::Serialize)]
pub struct ColumnInfo {
    pub name: String,
    /// The declared type text; empty for untyped columns.
    #[serde(rename = "type")]
    pub column_type: String,
    pub not_null: bool,
    pub primary_key: bool,
}

/// One user table's shape, for dashboards.
#[derive(serde::Serialize)]
pub struct TableInfo {
    pub name: String,
    pub rows: i64,
    pub columns: Vec<ColumnInfo>,
}

/// What the dashboard's database viewer reads in one call.
#[derive(serde::Serialize)]
pub struct Overview {
    /// The database file's size in bytes (page count times page size).
    pub size_bytes: i64,
    pub tables: Vec<TableInfo>,
}

/// The database's file size and user tables with their shapes, reusable
/// by any read path that can open the file (a snapshot, a replica, an api
/// endpoint) without dispatching at all. Reserved and internal tables
/// stay hidden. Column metadata comes from the platform connection, which
/// the script authorizer never restricts.
pub fn read_overview(storage: &mut crate::storage::SqliteStorage) -> Result<Overview, String> {
    let connection = storage.platform();

    let page_count: i64 = connection
        .pragma_query_value(None, "page_count", |row| row.get(0))
        .map_err(|e| e.to_string())?;
    let page_size: i64 = connection
        .pragma_query_value(None, "page_size", |row| row.get(0))
        .map_err(|e| e.to_string())?;

    let names: Vec<String> = {
        let mut statement = connection
            .prepare(
                "SELECT name FROM sqlite_master WHERE type = 'table' \
                 AND name NOT LIKE '__actias_%' AND name NOT LIKE 'sqlite_%' \
                 ORDER BY name",
            )
            .map_err(|e| e.to_string())?;
        statement
            .query_map([], |row| row.get(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?
    };

    let mut tables = Vec::new();
    for name in names {
        // Identifier interpolation is safe here: the name came from
        // sqlite_master, quoted against exotic table names.
        let quoted = name.replace('"', "\"\"");
        let rows = connection
            .query_row(&format!("SELECT COUNT(*) FROM \"{quoted}\""), [], |row| {
                row.get(0)
            })
            .map_err(|e| e.to_string())?;

        let columns = {
            let mut statement = connection
                .prepare(&format!("PRAGMA table_info(\"{quoted}\")"))
                .map_err(|e| e.to_string())?;
            statement
                .query_map([], |row| {
                    Ok(ColumnInfo {
                        name: row.get(1)?,
                        column_type: row.get(2)?,
                        not_null: row.get::<_, i64>(3)? != 0,
                        primary_key: row.get::<_, i64>(5)? != 0,
                    })
                })
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?
        };

        tables.push(TableInfo {
            name,
            rows,
            columns,
        });
    }

    Ok(Overview {
        size_bytes: page_count * page_size,
        tables,
    })
}
