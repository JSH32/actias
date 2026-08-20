//! The `__database` platform class: the sql product face, an ordinary
//! object whose methods are the storage surface.
//!
//! The statements are user SQL, so unlike the other platform classes
//! every one of them runs through the script-guarded [`SqliteStorage`]
//! surface, never the bare connection. Pending migrations apply before
//! the first statement of a vm's life; the tracking rows ride the call's
//! transaction, so a failed migration applies nothing, records nothing,
//! and retries on the next touch.

use crate::runtime::ActiasRuntime;
use crate::storage::SqliteStorage;

use crate::extensions::objects::DATABASE_CLASS;

/// Marks migrations as checked for this vm's life; the applied table in
/// the file is the durable record, this only skips re-reading it per call.
struct MigrationsChecked;

/// Routes one `__database` method call.
///
/// # Errors
/// Returns the user-safe text of whatever failed: a refused statement, a
/// failed migration, or SQLite's own message.
pub(crate) async fn dispatch(
    runtime: &ActiasRuntime,
    call: &super::Call,
) -> Result<serde_json::Value, String> {
    if runtime.app_data_ref::<MigrationsChecked>().is_none() {
        apply_migrations(runtime, &call.name)?;
        runtime.set_app_data(MigrationsChecked);
    }

    match call.method.as_str() {
        "exec" => {
            let (text, params) = statement(&call.args)?;
            super::with_storage(runtime, |storage| storage.exec(&text, &params))?;
            Ok(serde_json::Value::Bool(true))
        }
        "query" | "read" => {
            let (text, params) = statement(&call.args)?;
            let rows = super::with_storage(runtime, |storage| storage.query(&text, &params))?;
            Ok(serde_json::Value::Array(rows))
        }
        "query_one" | "read_one" => {
            let (text, params) = statement(&call.args)?;
            let rows = super::with_storage(runtime, |storage| storage.query(&text, &params))?;
            Ok(rows.into_iter().next().unwrap_or(serde_json::Value::Null))
        }
        // A batch is nothing special: one call is one transaction already,
        // so this is a loop with the atomicity coming from the dispatch
        // guard.
        "batch" => {
            let entries = call
                .args
                .first()
                .and_then(|value| value.as_array())
                .ok_or_else(|| "batch takes a list of statements.".to_owned())?;

            let mut affected = Vec::new();
            for entry in entries {
                let parts = entry
                    .as_array()
                    .ok_or_else(|| "Each batch entry is { sql, params }.".to_owned())?;
                let (text, params) = statement(parts)?;
                affected.push(super::with_storage(runtime, |storage| {
                    storage.exec(&text, &params)
                })?);
            }
            Ok(serde_json::json!(affected))
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

/// Applies this database's pending migrations in order.
fn apply_migrations(runtime: &ActiasRuntime, database: &str) -> Result<(), String> {
    let migrations = {
        let Some(prepared) =
            runtime.app_data_ref::<std::sync::Arc<crate::runtime::PreparedRevision>>()
        else {
            return Err("Runtime has no revision loaded.".to_owned());
        };
        prepared.migrations(database)
    };
    if migrations.is_empty() {
        return Ok(());
    }

    super::with_storage(runtime, |storage: &mut SqliteStorage| {
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
