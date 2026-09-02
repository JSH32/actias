//! The reserved directory tables in the object's own file: the local
//! truth every downstream copy derives from. Written on the call's
//! own connection so the write commits with the business transaction
//! and ships with the object's write-ahead log; the authorizer denies
//! `__actias_` tables to raw sql, so these verbs are the only door.
//!
//! The epoch is deliberately absent here: the file does not know its
//! placement epoch. The shipper attaches it from the lease when the
//! row leaves the file.

use crate::storage::SqliteStorage;

use super::shape::Value;

/// Field value kind spellings, matching the typed-pair encoding the
/// state store and kv service share so one rendering serves all three.
const KIND_STRING: &str = "string";
const KIND_INTEGER: &str = "integer";
const KIND_NUMBER: &str = "number";
const KIND_BOOLEAN: &str = "boolean";
const KIND_ARRAY: &str = "array";

/// The json text of a value: what an array field stores in the
/// overlay column and binds as a parameter, so `json_each` reads
/// either side identically.
pub(super) fn to_json_text(value: &Value) -> String {
    fn json(value: &Value) -> serde_json::Value {
        match value {
            Value::Text(text) => serde_json::Value::String(text.clone()),
            Value::Integer(number) => (*number).into(),
            Value::Number(number) => serde_json::Number::from_f64(*number)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null),
            Value::Bool(flag) => (*flag).into(),
            Value::Array(members) => serde_json::Value::Array(members.iter().map(json).collect()),
        }
    }
    json(value).to_string()
}

/// One field as it travels outside the object's file: the typed-pair
/// encoding again, so the manifest, the deltas and the console all
/// read one spelling. Values stay encoded here rather than decoded to
/// [`Value`] and re-encoded, which is both cheaper and lossless.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Pair {
    pub field: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub value: String,
}

/// The directory row as it leaves the object's file, riding the
/// shipping manifest. This is what makes every repair path a metadata
/// copy: the row is already beside the state it describes, so the
/// sweep and a full rebuild read manifests and never open an object.
///
/// The epoch is absent on purpose, here as in the file: the object
/// does not know its placement epoch. Whoever ships or repairs the row
/// attaches it from the lease.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct RowSnapshot {
    /// Directory evaluations this file has recorded.
    pub rev: i64,
    /// Field-set generation the fields were derived under. Zero until
    /// a class manifest gives generations a meaning; the merge order
    /// treats it as the lowest, so anything later supersedes it.
    pub dver: i64,
    /// The last good fields, in field-name order.
    pub fields: Vec<Pair>,
    /// The `(rev, dver)` of the most recent failed evaluation, when one
    /// is outstanding. Travels with the row so a console can count
    /// failures without opening anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<(i64, i64)>,
}

/// The current directory state of one object's file.
#[derive(Debug, PartialEq)]
pub struct StoredRow {
    /// Directory evaluations this file has recorded; advances on
    /// failures too, so a failure is shippable and orderable.
    pub rev: i64,
    /// Declaration version the fields were produced under.
    pub dver: i64,
    /// The last good fields, in field-name order. Survive a failed
    /// evaluation untouched: the row is never dropped.
    pub fields: Vec<(String, Value)>,
    /// The (rev, dver) of the most recent failed evaluation, cleared
    /// by the next success. Ships with the row so the console's
    /// failed counters and backfill can find it.
    pub failed: Option<(i64, i64)>,
}

/// Creates the reserved tables when absent; idempotent, called from
/// every verb that touches them.
fn ensure_tables(storage: &mut SqliteStorage) -> Result<(), String> {
    let connection = storage.platform();
    connection
        .execute(
            "CREATE TABLE IF NOT EXISTS __actias_directory (
                field TEXT PRIMARY KEY,
                type  TEXT NOT NULL,
                value TEXT NOT NULL
            )",
            [],
        )
        .map_err(|e| e.to_string())?;
    connection
        .execute(
            "CREATE TABLE IF NOT EXISTS __actias_directory_meta (
                id          INTEGER PRIMARY KEY CHECK (id = 1),
                rev         INTEGER NOT NULL,
                dver        INTEGER NOT NULL,
                failed_rev  INTEGER,
                failed_dver INTEGER
            )",
            [],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// One value as the travelling pair the manifest and deltas carry.
///
/// Public because a backfill derives rows outside the object's file and
/// so never goes through [`record`]: it needs the same encoding without
/// the storage write, and a second spelling of it would be a second
/// thing to keep in step.
pub fn encode_pair(value: &Value) -> (String, String) {
    let (kind, text) = encode(value);
    (kind.to_owned(), text)
}

fn encode(value: &Value) -> (&'static str, String) {
    match value {
        Value::Text(text) => (KIND_STRING, text.clone()),
        Value::Integer(number) => (KIND_INTEGER, number.to_string()),
        Value::Number(number) => (KIND_NUMBER, number.to_string()),
        Value::Bool(flag) => (KIND_BOOLEAN, flag.to_string()),
        Value::Array(_) => (KIND_ARRAY, to_json_text(value)),
    }
}

pub(super) fn decode(kind: &str, value: &str) -> Result<Value, String> {
    match kind {
        KIND_STRING => Ok(Value::Text(value.to_owned())),
        KIND_INTEGER => value
            .parse()
            .map(Value::Integer)
            .map_err(|_| format!("directory value '{value}' does not parse as an integer")),
        KIND_NUMBER => value
            .parse()
            .map(Value::Number)
            .map_err(|_| format!("directory value '{value}' does not parse as a number")),
        KIND_BOOLEAN => match value {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            other => Err(format!(
                "directory value '{other}' does not parse as a boolean"
            )),
        },
        KIND_ARRAY => {
            let parsed: serde_json::Value = serde_json::from_str(value)
                .map_err(|_| format!("directory value '{value}' does not parse as an array"))?;
            let serde_json::Value::Array(members) = parsed else {
                return Err(format!("directory value '{value}' is not a json array"));
            };
            let mut decoded = Vec::with_capacity(members.len());
            for member in members {
                decoded.push(match member {
                    serde_json::Value::String(text) => Value::Text(text),
                    serde_json::Value::Number(number) => match number.as_i64() {
                        Some(integer) => Value::Integer(integer),
                        None => Value::Number(number.as_f64().unwrap_or(0.0)),
                    },
                    serde_json::Value::Bool(flag) => Value::Bool(flag),
                    other => {
                        return Err(format!("array member '{other}' is not a scalar"));
                    }
                });
            }
            Ok(Value::Array(decoded))
        }
        other => Err(format!("unknown directory value kind '{other}'")),
    }
}

/// Records a successful evaluation: replaces the fields, advances the
/// rev, clears any failed marker. Returns the new rev.
///
/// Runs on the call's connection inside its transaction, so the row
/// and the business write commit or roll back together.
///
/// # Errors
/// Refuses a row whose encoded size passes
/// [`super::DEFAULT_ROW_MAX_BYTES`]; the evaluation layer contains
/// that refusal exactly like a throw, keeping the last good row.
pub fn record(
    storage: &mut SqliteStorage,
    dver: i64,
    fields: &[(String, Value)],
) -> Result<i64, String> {
    for (name, value) in fields {
        if let Value::Array(members) = value
            && members.iter().any(|member| !member.is_scalar())
        {
            return Err(format!(
                "directory field '{name}' nests an array inside an array; members are scalars"
            ));
        }
    }
    let encoded: usize = fields
        .iter()
        .map(|(name, value)| name.len() + encode(value).1.len())
        .sum();
    if encoded > super::DEFAULT_ROW_MAX_BYTES {
        return Err(format!(
            "directory row encodes to {encoded} bytes; the cap is {}",
            super::DEFAULT_ROW_MAX_BYTES
        ));
    }
    ensure_tables(storage)?;

    // An unchanged row writes nothing at all.
    //
    // The rev is the row's rank, so bumping it unconditionally makes
    // every write look like news: the syncer offers the row, the flush
    // encodes it, a delta uploads it and the compactor folds it, for a
    // row that says exactly what the index already holds. An object
    // exposing only fields it rarely touches would pay that on every
    // write it makes.
    //
    // The failure clause is load-bearing rather than an optimisation
    // detail: if the last derivation failed, the stored fields are the
    // last good ones and identical fields must still be written,
    // because the write is what clears the marker.
    // Compared by name rather than pairwise: `record` promises no
    // ordering of its input, and a caller that happened to pass a
    // different order would otherwise never skip, which is a silent
    // regression rather than a visible bug. Rows carry few fields, so
    // the lookup is cheaper than sorting a copy.
    if let Some(stored) = current(storage)?
        && stored.failed.is_none()
        && stored.dver == dver
        && stored.fields.len() == fields.len()
        && fields.iter().all(|(name, value)| {
            stored
                .fields
                .iter()
                .any(|(held, was)| held == name && was == value)
        })
    {
        return Ok(stored.rev);
    }

    let connection = storage.platform();
    let rev: i64 = connection
        .query_row(
            "INSERT INTO __actias_directory_meta (id, rev, dver, failed_rev, failed_dver)
             VALUES (1, 1, ?, NULL, NULL)
             ON CONFLICT (id) DO UPDATE SET
                 rev = __actias_directory_meta.rev + 1, dver = excluded.dver,
                 failed_rev = NULL, failed_dver = NULL
             RETURNING rev",
            [dver],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    connection
        .execute("DELETE FROM __actias_directory", [])
        .map_err(|e| e.to_string())?;
    for (name, value) in fields {
        let (kind, text) = encode(value);
        connection
            .execute(
                "INSERT INTO __actias_directory (field, type, value) VALUES (?, ?, ?)",
                rusqlite::params![name, kind, text],
            )
            .map_err(|e| e.to_string())?;
    }
    Ok(rev)
}

/// Records a failed evaluation: the fields stay untouched (the last
/// good row is never dropped), the rev still advances so the failure
/// is orderable, and the marker names what failed. Returns the rev.
///
/// # Errors
/// Returns SQLite's message.
pub fn record_failure(storage: &mut SqliteStorage, dver: i64) -> Result<i64, String> {
    ensure_tables(storage)?;
    storage
        .platform()
        .query_row(
            "INSERT INTO __actias_directory_meta (id, rev, dver, failed_rev, failed_dver)
             VALUES (1, 1, ?, 1, ?)
             ON CONFLICT (id) DO UPDATE SET
                 rev = __actias_directory_meta.rev + 1,
                 failed_rev = __actias_directory_meta.rev + 1, failed_dver = excluded.failed_dver
             RETURNING rev",
            rusqlite::params![dver, dver],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())
}

/// The file's current directory state, or [`None`] where no
/// evaluation ever ran. Tolerates a file the verbs never touched (a
/// read-only open cannot create tables), reading absence as absence.
///
/// # Errors
/// Returns SQLite's message.
pub fn current(storage: &mut SqliteStorage) -> Result<Option<StoredRow>, String> {
    let connection = storage.platform();
    let meta = connection.query_row(
        "SELECT rev, dver, failed_rev, failed_dver FROM __actias_directory_meta WHERE id = 1",
        [],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<i64>>(3)?,
            ))
        },
    );
    let (rev, dver, failed_rev, failed_dver) = match meta {
        Ok(values) => values,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(rusqlite::Error::SqliteFailure(_, Some(text))) if text.contains("no such table") => {
            return Ok(None);
        }
        Err(other) => return Err(other.to_string()),
    };
    let mut statement = connection
        .prepare("SELECT field, type, value FROM __actias_directory ORDER BY field")
        .map_err(|e| e.to_string())?;
    let pairs = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let mut fields = Vec::with_capacity(pairs.len());
    for (name, kind, text) in pairs {
        fields.push((name, decode(&kind, &text)?));
    }
    Ok(Some(StoredRow {
        rev,
        dver,
        fields,
        failed: failed_rev.zip(failed_dver),
    }))
}

/// The travelling pairs as kernel values, for re-running a predicate
/// against a manifest's row in memory. The verified read leans on this:
/// a settled row that still matches needs no restore, only this decode
/// and [`super::verify::matches`].
///
/// # Errors
/// A pair whose value does not parse as its kind names itself; the
/// caller treats the row as unverifiable rather than guessing.
pub fn decode_pairs(pairs: &[Pair]) -> Result<Vec<(String, Value)>, String> {
    let mut fields = Vec::with_capacity(pairs.len());
    for pair in pairs {
        fields.push((pair.field.clone(), decode(&pair.kind, &pair.value)?));
    }
    Ok(fields)
}

/// The row in its travelling form, or [`None`] where no evaluation
/// ever ran. Reads the encoded pairs directly, so a value the current
/// kernel could not decode still ships rather than failing a flight:
/// the row is an index, and refusing to carry it would cost freshness
/// for no gain.
///
/// Tolerates a file the verbs never touched, like [`current`], because
/// the shipper opens files that may predate the directory entirely.
///
/// # Errors
/// Returns SQLite's message.
pub fn snapshot(storage: &mut SqliteStorage) -> Result<Option<RowSnapshot>, String> {
    let connection = storage.platform();
    let meta = connection.query_row(
        "SELECT rev, dver, failed_rev, failed_dver FROM __actias_directory_meta WHERE id = 1",
        [],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<i64>>(3)?,
            ))
        },
    );
    let (rev, dver, failed_rev, failed_dver) = match meta {
        Ok(values) => values,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(rusqlite::Error::SqliteFailure(_, Some(text))) if text.contains("no such table") => {
            return Ok(None);
        }
        Err(other) => return Err(other.to_string()),
    };

    let mut statement = connection
        .prepare("SELECT field, type, value FROM __actias_directory ORDER BY field")
        .map_err(|e| e.to_string())?;
    let fields = statement
        .query_map([], |row| {
            Ok(Pair {
                field: row.get(0)?,
                kind: row.get(1)?,
                value: row.get(2)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    Ok(Some(RowSnapshot {
        rev,
        dver,
        fields,
        failed: failed_rev.zip(failed_dver),
    }))
}

#[cfg(test)]
mod tests {
    use crate::storage::SqliteStorage;

    use super::super::shape::Value;
    use super::{current, record, record_failure};

    fn fields() -> Vec<(String, Value)> {
        vec![
            ("status".to_owned(), Value::Text("open".to_owned())),
            ("high_bid".to_owned(), Value::Integer(25)),
            ("score".to_owned(), Value::Number(0.75)),
            ("featured".to_owned(), Value::Bool(true)),
            (
                "tags".to_owned(),
                Value::Array(vec![
                    Value::Text("vintage".to_owned()),
                    Value::Integer(7),
                    Value::Bool(false),
                ]),
            ),
        ]
    }

    #[test]
    fn a_fresh_file_reads_as_absent() {
        let mut storage = SqliteStorage::in_memory().unwrap();
        assert_eq!(current(&mut storage).unwrap(), None);
    }

    #[test]
    fn record_replaces_and_advances() {
        let mut storage = SqliteStorage::in_memory().unwrap();
        assert_eq!(record(&mut storage, 1, &fields()).unwrap(), 1);
        assert_eq!(record(&mut storage, 1, &fields()[..1]).unwrap(), 2);
        let row = current(&mut storage).unwrap().unwrap();
        assert_eq!(row.rev, 2);
        assert_eq!(row.fields.len(), 1);
        assert_eq!(row.failed, None);
    }

    /// The rev is the row's rank downstream, so bumping it for a row
    /// that says nothing new makes the syncer offer it, a delta carry
    /// it and the compactor fold it, all to restate what the index
    /// already holds.
    #[test]
    fn an_unchanged_row_writes_nothing() {
        let mut storage = SqliteStorage::in_memory().unwrap();
        assert_eq!(record(&mut storage, 1, &fields()).unwrap(), 1);

        // Sorted the same way on both sides, so an identical row is
        // recognised whatever order the derive returned it in.
        assert_eq!(
            record(&mut storage, 1, &fields()).unwrap(),
            1,
            "the rev holds, so nothing downstream sees news"
        );
        assert_eq!(current(&mut storage).unwrap().unwrap().rev, 1);

        // A real change still advances.
        assert_eq!(record(&mut storage, 1, &fields()[..2]).unwrap(), 2);
    }

    /// A row whose values are unchanged but whose declaration moved is
    /// news: the backfill exists to move exactly this, and holding the
    /// rev would leave the floor where it was.
    #[test]
    fn the_same_fields_at_a_new_declaration_still_advance() {
        let mut storage = SqliteStorage::in_memory().unwrap();
        record(&mut storage, 1, &fields()).unwrap();
        assert_eq!(record(&mut storage, 2, &fields()).unwrap(), 2);
        assert_eq!(current(&mut storage).unwrap().unwrap().dver, 2);
    }

    /// The load-bearing half of the skip: after a failure the stored
    /// fields are the last good ones, so an identical derive must still
    /// write, because the write is what clears the marker.
    #[test]
    fn an_unchanged_row_after_a_failure_still_writes() {
        let mut storage = SqliteStorage::in_memory().unwrap();
        record(&mut storage, 1, &fields()).unwrap();
        record_failure(&mut storage, 1).unwrap();
        assert!(current(&mut storage).unwrap().unwrap().failed.is_some());

        let rev = record(&mut storage, 1, &fields()).unwrap();
        let row = current(&mut storage).unwrap().unwrap();
        assert_eq!(row.rev, rev);
        assert_eq!(
            row.failed, None,
            "the identical row is what clears the failure"
        );
    }

    #[test]
    fn failure_keeps_the_last_good_row_and_stays_orderable() {
        let mut storage = SqliteStorage::in_memory().unwrap();
        record(&mut storage, 1, &fields()).unwrap();
        let failed_rev = record_failure(&mut storage, 2).unwrap();
        assert_eq!(failed_rev, 2);
        let row = current(&mut storage).unwrap().unwrap();
        assert_eq!(
            row.fields,
            fields()
                .into_iter()
                .collect::<std::collections::BTreeMap<_, _>>()
                .into_iter()
                .collect::<Vec<_>>()
        );
        assert_eq!(row.failed, Some((2, 2)));
        // The next success clears the marker and keeps advancing.
        assert_eq!(record(&mut storage, 2, &fields()).unwrap(), 3);
        assert_eq!(current(&mut storage).unwrap().unwrap().failed, None);
    }

    #[test]
    fn values_round_trip_through_the_typed_pairs() {
        let mut storage = SqliteStorage::in_memory().unwrap();
        record(&mut storage, 1, &fields()).unwrap();
        let row = current(&mut storage).unwrap().unwrap();
        let find = |name: &str| {
            row.fields
                .iter()
                .find(|(n, _)| n == name)
                .unwrap()
                .1
                .clone()
        };
        // Integers stay integers: no float formatting drift ("25.0").
        assert_eq!(find("high_bid"), Value::Integer(25));
        assert_eq!(find("score"), Value::Number(0.75));
        assert_eq!(find("featured"), Value::Bool(true));
        // Arrays keep members and their kinds through the json text.
        assert_eq!(
            find("tags"),
            Value::Array(vec![
                Value::Text("vintage".to_owned()),
                Value::Integer(7),
                Value::Bool(false),
            ])
        );
    }

    #[test]
    fn a_nested_array_refuses() {
        let mut storage = SqliteStorage::in_memory().unwrap();
        let nested = vec![(
            "tags".to_owned(),
            Value::Array(vec![Value::Array(vec![Value::Integer(1)])]),
        )];
        let error = record(&mut storage, 1, &nested).unwrap_err();
        assert!(error.contains("scalars"), "{error}");
    }

    #[test]
    fn the_snapshot_carries_what_a_repair_needs() {
        let mut storage = SqliteStorage::in_memory().unwrap();
        // A file the verbs never touched carries no row, and reading it
        // must not create tables: the shipper opens files that predate
        // the directory entirely.
        assert_eq!(super::snapshot(&mut storage).unwrap(), None);

        record(&mut storage, 3, &fields()).unwrap();
        let snapshot = super::snapshot(&mut storage)
            .unwrap()
            .expect("a row exists");
        assert_eq!(snapshot.rev, 1);
        assert_eq!(snapshot.dver, 3);
        assert_eq!(snapshot.failed, None);
        // Encoded pairs, in field order, so the manifest and the
        // console read one spelling.
        let names: Vec<&str> = snapshot.fields.iter().map(|p| p.field.as_str()).collect();
        assert_eq!(
            names,
            vec!["featured", "high_bid", "score", "status", "tags"]
        );
        let tags = snapshot.fields.iter().find(|p| p.field == "tags").unwrap();
        assert_eq!(tags.kind, "array");

        // A failure travels too, so a console counts failures without
        // opening an object.
        record_failure(&mut storage, 3).unwrap();
        let after = super::snapshot(&mut storage)
            .unwrap()
            .expect("a row exists");
        assert_eq!(after.failed, Some((2, 3)));
        assert_eq!(
            after.fields, snapshot.fields,
            "the last good fields still ship"
        );
    }

    #[test]
    fn an_oversized_row_refuses_and_keeps_the_incumbent() {
        let mut storage = SqliteStorage::in_memory().unwrap();
        record(&mut storage, 1, &fields()).unwrap();
        let huge = vec![(
            "blob".to_owned(),
            Value::Text("x".repeat(super::super::DEFAULT_ROW_MAX_BYTES)),
        )];
        let error = record(&mut storage, 1, &huge).unwrap_err();
        assert!(error.contains("cap"), "{error}");
        // The incumbent row survives; only the attempt was refused.
        let row = current(&mut storage).unwrap().unwrap();
        assert_eq!(row.fields.len(), fields().len());
    }
}
