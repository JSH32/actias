//! The delta file: settled rows as bytes, ready to be named by their
//! content and uploaded.
//!
//! A delta is a SQLite file like everything else the platform stores,
//! so a compactor merges it with `ON CONFLICT` rather than hand-rolled
//! merge code, and an operator can open one. Rows and fields are
//! separate tables because fields travel as names: a delta carries
//! the publish's declared field set beside its rows, and a class's
//! manifest is what maps those names onto overlay columns.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use super::row::{Pair, RowSnapshot};

/// One object's contribution to a delta.
#[derive(Clone, Debug, PartialEq)]
pub struct DeltaRow {
    pub object_id: String,
    /// The instance name, so a listing answers with names rather than
    /// hashes.
    pub name: String,
    /// From the lease the flight shipped under; the object's own file
    /// does not know its epoch.
    pub epoch: u64,
    pub snapshot: RowSnapshot,
    /// A destroyed object contributes a tombstone instead of a row.
    pub tombstone: bool,
}

/// Names one scratch file per encode. A counter rather than a clock:
/// two concurrent encodes only need to differ, and a counter cannot
/// collide when a clock is coarse.
static SCRATCH: AtomicU64 = AtomicU64::new(0);

/// Encodes rows into delta bytes, written through a scratch file in
/// `scratch` and removed before returning.
///
/// Byte-for-byte deterministic in the rows it is given: they are
/// written in object-id order, so the same set always produces the
/// same bytes and therefore the same content-addressed name. That is
/// what makes a retried upload idempotent and two nodes unable to
/// collide.
///
/// # Errors
/// Returns SQLite's or the filesystem's message.
pub fn encode(
    rows: &[DeltaRow],
    declaration: Option<&actias_common::directory_spec::DirectorySpec>,
    scratch: &Path,
) -> Result<Vec<u8>, String> {
    let path = scratch.join(format!(
        "directory-delta-{}.sqlite",
        SCRATCH.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_file(&path);

    let encoded = write_delta(rows, declaration, &path);
    let bytes = encoded.and_then(|()| std::fs::read(&path).map_err(|e| e.to_string()));
    let _ = std::fs::remove_file(&path);
    bytes
}

fn write_delta(
    rows: &[DeltaRow],
    declaration: Option<&actias_common::directory_spec::DirectorySpec>,
    path: &Path,
) -> Result<(), String> {
    let connection = rusqlite::Connection::open(path).map_err(|e| e.to_string())?;
    // A scratch file, written whole and read straight back into bytes
    // whose content hash names them: the durability pragmas would only
    // fsync a file that is about to be deleted, and one transaction
    // around the rows is the difference between one write and one
    // fsync per row. Without it a 1M-row encode runs for hours at 4%
    // cpu, waiting on the disk once per row (measured).
    connection
        .execute_batch(
            "PRAGMA journal_mode = OFF;
             PRAGMA synchronous = OFF;
             BEGIN;
             CREATE TABLE rows (
                 object_id TEXT PRIMARY KEY,
                 name      TEXT NOT NULL,
                 epoch     INTEGER NOT NULL,
                 rev       INTEGER NOT NULL,
                 dver      INTEGER NOT NULL,
                 tombstone INTEGER NOT NULL,
                 failed    INTEGER NOT NULL
             );
             CREATE TABLE fields (
                 object_id TEXT NOT NULL,
                 field     TEXT NOT NULL,
                 type      TEXT NOT NULL,
                 value     TEXT NOT NULL,
                 PRIMARY KEY (object_id, field)
             );
             CREATE TABLE declaration (
                 dver  INTEGER NOT NULL,
                 field TEXT NOT NULL,
                 kind  TEXT NOT NULL
             );",
        )
        .map_err(|e| e.to_string())?;

    // The declaration the rows were derived under, when the writer had
    // one. This is how the manifest learns field sets from publishes
    // rather than inferring them from row arrival, which is what makes
    // a field's `since` exact instead of order-dependent. Spec fields
    // are already sorted, so the bytes stay deterministic.
    if let Some(spec) = declaration {
        for (field, kind) in &spec.fields {
            connection
                .execute(
                    "INSERT INTO declaration (dver, field, kind) VALUES (?, ?, ?)",
                    rusqlite::params![spec.dver as i64, field, kind],
                )
                .map_err(|e| e.to_string())?;
        }
    }

    let mut ordered: Vec<&DeltaRow> = rows.iter().collect();
    ordered.sort_by(|left, right| left.object_id.cmp(&right.object_id));

    for row in ordered {
        connection
            .execute(
                "INSERT INTO rows (object_id, name, epoch, rev, dver, tombstone, failed)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![
                    row.object_id,
                    row.name,
                    row.epoch as i64,
                    row.snapshot.rev,
                    row.snapshot.dver,
                    row.tombstone as i64,
                    row.snapshot.failed.is_some() as i64,
                ],
            )
            .map_err(|e| e.to_string())?;
        for pair in &row.snapshot.fields {
            connection
                .execute(
                    "INSERT INTO fields (object_id, field, type, value) VALUES (?, ?, ?, ?)",
                    rusqlite::params![row.object_id, pair.field, pair.kind, pair.value],
                )
                .map_err(|e| e.to_string())?;
        }
    }
    connection
        .execute_batch("COMMIT;")
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// What one delta holds: its rows, and the declaration they were
/// derived under (absent on repair deltas, which copy rows without
/// knowing their publish).
pub type ReadDelta = (
    Vec<DeltaRow>,
    Option<actias_common::directory_spec::DirectorySpec>,
);

/// Reads delta bytes back, through a scratch file in `scratch`. The
/// compactor's input side, and what lets anything else (a test, an
/// operator tool) open a delta without knowing its schema.
///
/// # Errors
/// Returns SQLite's or the filesystem's message.
pub fn read(bytes: &[u8], scratch: &Path) -> Result<ReadDelta, String> {
    let path = scratch.join(format!(
        "directory-read-{}.sqlite",
        SCRATCH.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_file(&path);
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;

    let rows = read_delta(&path);
    let _ = std::fs::remove_file(&path);
    rows
}

fn read_delta(path: &Path) -> Result<ReadDelta, String> {
    let connection = rusqlite::Connection::open(path).map_err(|e| e.to_string())?;

    let mut fields: std::collections::HashMap<String, Vec<Pair>> = std::collections::HashMap::new();
    {
        let mut statement = connection
            .prepare("SELECT object_id, field, type, value FROM fields ORDER BY object_id, field")
            .map_err(|e| e.to_string())?;
        let read = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    Pair {
                        field: row.get(1)?,
                        kind: row.get(2)?,
                        value: row.get(3)?,
                    },
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        for (object_id, pair) in read {
            fields.entry(object_id).or_default().push(pair);
        }
    }

    let mut statement = connection
        .prepare(
            "SELECT object_id, name, epoch, rev, dver, tombstone, failed
             FROM rows ORDER BY object_id",
        )
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([], |row| {
            let object_id: String = row.get(0)?;
            let failed: i64 = row.get(6)?;
            let rev: i64 = row.get(3)?;
            let dver: i64 = row.get(4)?;
            Ok(DeltaRow {
                name: row.get(1)?,
                epoch: row.get::<_, i64>(2)?.max(0) as u64,
                snapshot: RowSnapshot {
                    rev,
                    dver,
                    fields: Vec::new(),
                    // The pair is recoverable: a failure is always
                    // marked at the rev and dver the row carries.
                    failed: (failed != 0).then_some((rev, dver)),
                },
                tombstone: row.get::<_, i64>(5)? != 0,
                object_id,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    let rows: Vec<DeltaRow> = rows
        .into_iter()
        .map(|mut row| {
            row.snapshot.fields = fields.remove(&row.object_id).unwrap_or_default();
            row
        })
        .collect();

    // A delta from before declarations existed has no such table;
    // tolerating that read costs one prepare and keeps every delta in
    // the store readable, which repair depends on.
    let declaration = match connection.prepare("SELECT dver, field, kind FROM declaration") {
        Err(_) => None,
        Ok(mut statement) => {
            let entries = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?.max(0) as u64,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            entries.first().map(|(dver, _, _)| {
                actias_common::directory_spec::DirectorySpec::new(
                    *dver,
                    entries
                        .iter()
                        .map(|(_, field, kind)| (field.clone(), kind.clone()))
                        .collect(),
                )
            })
        }
    };

    Ok((rows, declaration))
}

#[cfg(test)]
mod tests {
    use super::super::row::Pair;
    use super::*;

    fn row(object_id: &str, rev: i64, status: &str) -> DeltaRow {
        DeltaRow {
            object_id: object_id.to_owned(),
            name: format!("name-{object_id}"),
            epoch: 5,
            snapshot: RowSnapshot {
                rev,
                dver: 0,
                fields: vec![Pair {
                    field: "status".to_owned(),
                    kind: "string".to_owned(),
                    value: status.to_owned(),
                }],
                failed: None,
            },
            tombstone: false,
        }
    }

    #[test]
    fn rows_and_their_fields_survive_the_round_trip() {
        let scratch = tempfile::tempdir().expect("tempdir");
        let declared = actias_common::directory_spec::DirectorySpec::new(
            3,
            vec![("status".to_owned(), "string".to_owned())],
        );
        let bytes = encode(
            &[row("obj-a", 2, "open"), row("obj-b", 1, "closed")],
            Some(&declared),
            scratch.path(),
        )
        .expect("encodes");

        let (rows, declaration) = read(&bytes, scratch.path()).expect("reads back");
        assert_eq!(
            rows,
            vec![row("obj-a", 2, "open"), row("obj-b", 1, "closed")]
        );
        // The publish rides with the rows it derived, which is how the
        // manifest learns a field set without inferring one.
        assert_eq!(declaration, Some(declared));
    }

    #[test]
    fn the_same_rows_encode_to_the_same_bytes() {
        let scratch = tempfile::tempdir().expect("tempdir");
        // Insertion order differs; content addressing must not.
        let first = encode(
            &[row("obj-a", 2, "open"), row("obj-b", 1, "closed")],
            None,
            scratch.path(),
        )
        .expect("encodes");
        let second = encode(
            &[row("obj-b", 1, "closed"), row("obj-a", 2, "open")],
            None,
            scratch.path(),
        )
        .expect("encodes");
        assert_eq!(first, second);

        let changed = encode(
            &[row("obj-a", 3, "open"), row("obj-b", 1, "closed")],
            None,
            scratch.path(),
        )
        .expect("encodes");
        assert_ne!(first, changed, "a changed row must change the name");
    }

    #[test]
    fn a_tombstone_carries_no_fields() {
        let scratch = tempfile::tempdir().expect("tempdir");
        let mut dead = row("obj-a", 7, "open");
        dead.tombstone = true;
        dead.snapshot = RowSnapshot::default();

        let bytes = encode(&[dead.clone()], None, scratch.path()).expect("encodes");
        let (rows, _) = read(&bytes, scratch.path()).expect("reads back");
        assert_eq!(rows, vec![dead]);
    }

    #[test]
    fn an_empty_delta_is_still_a_valid_file() {
        let scratch = tempfile::tempdir().expect("tempdir");
        let bytes = encode(&[], None, scratch.path()).expect("encodes");
        let (rows, declaration) = read(&bytes, scratch.path()).expect("reads back");
        assert!(rows.is_empty());
        // A repair delta declares nothing; absent must read as absent
        // rather than as an empty field set that would wipe a manifest.
        assert!(declaration.is_none());
    }
}
