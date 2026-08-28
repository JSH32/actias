//! The key-value face on an object's storage (docs/OBJECT-STATE.md):
//! one reserved table in the object's own file, written in the call's
//! own transaction. The verbs are the only door; the authorizer denies
//! the table to raw sql from scripts and the console alike, so this
//! module owns the representation completely.

use crate::storage::SqliteStorage;

/// Largest stored value, as encoded text. Past this, the state wanted
/// a table.
pub const VALUE_CAP_BYTES: usize = 64 * 1024;

/// Entries one `list` page carries when the caller does not say.
pub const LIST_DEFAULT_LIMIT: i64 = 100;
/// The most entries one `list` page may carry.
pub const LIST_MAX_LIMIT: i64 = 1000;

/// The typed-pair encoding the kv service uses: how `value` parses.
/// Sharing the encoding is what lets one rendering serve project kv
/// and object state alike.
#[derive(Debug, PartialEq, Eq)]
pub struct Pair {
    pub key: String,
    pub kind: String,
    pub value: String,
}

/// Creates the state table when absent; idempotent, called from every
/// verb that touches it.
fn ensure_table(storage: &mut SqliteStorage) -> Result<(), String> {
    storage
        .platform()
        .execute(
            "CREATE TABLE IF NOT EXISTS __actias_state (
                key   TEXT PRIMARY KEY,
                type  TEXT NOT NULL,
                value TEXT NOT NULL
            )",
            [],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// One key's typed pair, or [`None`] where nothing is stored.
pub fn get(storage: &mut SqliteStorage, key: &str) -> Result<Option<Pair>, String> {
    ensure_table(storage)?;
    storage
        .platform()
        .query_row(
            "SELECT type, value FROM __actias_state WHERE key = ?",
            [key],
            |row| {
                Ok(Pair {
                    key: key.to_owned(),
                    kind: row.get(0)?,
                    value: row.get(1)?,
                })
            },
        )
        .map(Some)
        .or_else(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other.to_string()),
        })
}

/// Writes one typed pair; setting replaces.
///
/// # Errors
/// Refuses a value past [`VALUE_CAP_BYTES`], naming the key.
pub fn set(storage: &mut SqliteStorage, key: &str, kind: &str, value: &str) -> Result<(), String> {
    if value.len() > VALUE_CAP_BYTES {
        return Err(format!(
            "Value for key '{key}' is {} bytes; the store caps at {VALUE_CAP_BYTES}. \
             Past this, the state wants a table (state.sql).",
            value.len()
        ));
    }
    ensure_table(storage)?;
    storage
        .platform()
        .execute(
            "INSERT INTO __actias_state (key, type, value) VALUES (?, ?, ?)
             ON CONFLICT (key) DO UPDATE SET type = excluded.type, value = excluded.value",
            rusqlite::params![key, kind, value],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Removes one key; absent is fine.
pub fn delete(storage: &mut SqliteStorage, key: &str) -> Result<(), String> {
    ensure_table(storage)?;
    storage
        .platform()
        .execute("DELETE FROM __actias_state WHERE key = ?", [key])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// One page of pairs in ascending key order, kv's exact page shape: the
/// cursor names where the next page starts, and a page without one is
/// the last. `prefix` narrows; empty matches everything.
pub fn list(
    storage: &mut SqliteStorage,
    prefix: &str,
    limit: i64,
    cursor: Option<&str>,
) -> Result<(Vec<Pair>, Option<String>), String> {
    ensure_table(storage)?;
    let limit = limit.clamp(1, LIST_MAX_LIMIT);
    let like = format!(
        "{}%",
        prefix
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );

    let connection = storage.platform();
    let mut statement = connection
        .prepare(
            "SELECT key, type, value FROM __actias_state
             WHERE key LIKE ? ESCAPE '\\' AND key > ?
             ORDER BY key LIMIT ?",
        )
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map(
            rusqlite::params![like, cursor.unwrap_or(""), limit + 1],
            |row| {
                Ok(Pair {
                    key: row.get(0)?,
                    kind: row.get(1)?,
                    value: row.get(2)?,
                })
            },
        )
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    let mut pairs = rows;
    let next = if pairs.len() as i64 > limit {
        pairs.truncate(limit as usize);
        pairs.last().map(|pair| pair.key.clone())
    } else {
        None
    };
    Ok((pairs, next))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_round_trip_and_pages_walk_in_order() {
        let mut storage = SqliteStorage::in_memory().expect("opens");

        assert_eq!(get(&mut storage, "phase").expect("reads"), None);
        set(&mut storage, "phase", "string", "open").expect("writes");
        set(&mut storage, "count", "integer", "3").expect("writes");
        set(&mut storage, "meta:a", "json", "{\"x\":1}").expect("writes");
        set(&mut storage, "meta:b", "json", "{\"x\":2}").expect("writes");

        let phase = get(&mut storage, "phase").expect("reads").expect("stored");
        assert_eq!(
            (phase.kind.as_str(), phase.value.as_str()),
            ("string", "open")
        );

        // Setting replaces; deleting removes.
        set(&mut storage, "phase", "string", "closed").expect("writes");
        assert_eq!(
            get(&mut storage, "phase")
                .expect("reads")
                .expect("stored")
                .value,
            "closed"
        );
        delete(&mut storage, "count").expect("deletes");
        assert_eq!(get(&mut storage, "count").expect("reads"), None);

        // Pages walk every key in order; the prefix narrows; the last
        // page carries no cursor.
        let (page, next) = list(&mut storage, "", 2, None).expect("lists");
        assert_eq!(
            page.iter().map(|p| p.key.as_str()).collect::<Vec<_>>(),
            vec!["meta:a", "meta:b"]
        );
        let cursor = next.expect("more pages");
        let (rest, done) = list(&mut storage, "", 2, Some(&cursor)).expect("lists");
        assert_eq!(rest[0].key, "phase");
        assert!(done.is_none());
        let (narrowed, _) = list(&mut storage, "meta:", 10, None).expect("lists");
        assert_eq!(narrowed.len(), 2);
    }

    #[test]
    fn an_oversized_value_is_refused_naming_the_key() {
        let mut storage = SqliteStorage::in_memory().expect("opens");
        let big = "x".repeat(VALUE_CAP_BYTES + 1);
        let refused = set(&mut storage, "blob", "string", &big).expect_err("must refuse");
        assert!(
            refused.contains("'blob'") && refused.contains("state.sql"),
            "{refused}"
        );
    }

    #[test]
    fn the_reserved_table_stays_invisible_to_script_sql() {
        let mut storage = SqliteStorage::in_memory().expect("opens");
        set(&mut storage, "secretish", "string", "hidden").expect("writes");

        for refused in ["SELECT * FROM __actias_state", "DELETE FROM __actias_state"] {
            let error = storage
                .exec(refused, &[])
                .expect_err("the authorizer must refuse");
            assert!(
                error.contains("authoriz") || error.contains("prohibited"),
                "{refused:?}: {error}"
            );
        }
    }
}
