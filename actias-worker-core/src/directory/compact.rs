//! Merging a base and the deltas that arrived since into a new base.
//!
//! The merge is last-writer-wins on `(epoch, rev, dver)`, which is what
//! makes deltas an unordered bag: nothing here depends on file names,
//! arrival order, or which node wrote what. A duplicated delta merges
//! to the same result, and a delta that arrives late from an older
//! residency loses on epoch.
//!
//! It also computes what the class's manifest records: the fields seen,
//! and the lowest dver across every row, which together decide when a
//! newly discovered field becomes queryable.

use std::collections::HashMap;
use std::path::Path;

use super::delta::{self, DeltaRow};
use super::manifest::Manifest;

/// What one merge produced.
pub struct Merged {
    /// The new base's bytes, ready to be named by their content.
    pub bytes: Vec<u8>,
    /// Live rows it holds, tombstones excluded.
    pub rows: u64,
    /// The identity checksum of those live rows, which the manifest
    /// carries so reconciliation can compare it against the placement
    /// store's fold over the identities that exist.
    pub identities: i64,
    /// The lowest dver across those rows, or the manifest's current
    /// field-set generation when there are none: an empty class has
    /// nothing left to backfill, so nothing should read as building.
    pub min_dver: u64,
    /// Declarations carried by the folded deltas, for the manifest to
    /// fold in via `observe_declaration`. Field sets come from
    /// publishes, never from scraping rows: a row's absence of a field
    /// is a legal value state (nil is absent), so rows cannot
    /// distinguish "field added later" from "field absent here", and
    /// inferring a field set from them makes `since` depend on delta
    /// arrival order.
    pub declarations: Vec<actias_common::directory_spec::DirectorySpec>,
}

/// Ranks one row against another for the same object. Epoch first, then
/// a tombstone over a live row at the same epoch (destruction is final
/// within its epoch, and a recreation always claims a higher one), then
/// rev and dver.
fn rank(row: &DeltaRow) -> (u64, bool, u64, u64) {
    (
        row.epoch,
        row.tombstone,
        row.snapshot.rev.max(0) as u64,
        row.snapshot.dver.max(0) as u64,
    )
}

/// Merges `base` (absent before the first compaction) with `deltas`,
/// newest row per object winning, and reports what the manifest needs.
///
/// Tombstones are kept in the output rather than dropped. Deltas are
/// unordered and one can always arrive late, so a dropped tombstone
/// would let a stale row resurrect an object that no longer exists;
/// keeping it costs one small row per destroyed object, and collecting
/// them is a placement-driven sweep rather than a merge-time guess.
///
/// # Errors
/// Returns SQLite's or the filesystem's message.
pub fn merge<D: AsRef<[u8]>>(
    base: Option<&[u8]>,
    deltas: &[D],
    manifest: &Manifest,
    scratch: &Path,
) -> Result<Merged, String> {
    let mut winners: HashMap<String, DeltaRow> = HashMap::new();
    let mut declarations = Vec::new();

    let mut absorb = |rows: Vec<DeltaRow>| {
        for row in rows {
            match winners.get(&row.object_id) {
                Some(existing) if rank(existing) >= rank(&row) => {}
                _ => {
                    winners.insert(row.object_id.clone(), row);
                }
            }
        }
    };

    if let Some(base) = base {
        // The base re-encodes without a declaration: the manifest is
        // what accumulates field sets, so the base only carries rows.
        let (rows, _) = delta::read(base, scratch)?;
        absorb(rows);
    }
    for bytes in deltas {
        let (rows, declaration) = delta::read(bytes.as_ref(), scratch)?;
        absorb(rows);
        if let Some(spec) = declaration {
            declarations.push(spec);
        }
    }

    let mut merged: Vec<DeltaRow> = winners.into_values().collect();
    merged.sort_by(|left, right| left.object_id.cmp(&right.object_id));

    // The floor comes from derived live rows only. A tombstone has no
    // fields, and a placeholder (rev 0: an identity repair found with
    // nothing ever derived) has no state to re-derive; letting either
    // one's zero dver set the floor would keep every field building
    // forever. The placeholder still counts as a row and as an
    // identity, because existing is what it records.
    let mut min_dver = None;
    let mut rows = 0u64;
    let mut identities = 0i64;
    for row in &merged {
        if row.tombstone {
            continue;
        }
        rows += 1;
        identities ^= super::identity::contribution(&row.object_id);
        if row.snapshot.rev == 0 {
            continue;
        }
        let dver = row.snapshot.dver.max(0) as u64;
        min_dver = Some(min_dver.map_or(dver, |low: u64| low.min(dver)));
    }

    Ok(Merged {
        bytes: delta::encode(&merged, None, scratch)?,
        rows,
        identities,
        min_dver: min_dver.unwrap_or(manifest.dver),
        declarations,
    })
}

#[cfg(test)]
mod tests {
    use super::super::row::{Pair, RowSnapshot};
    use super::*;

    /// A merge with no deltas, spelled so the byte holder's type is
    /// named: the parameter is generic over anything that lends bytes.
    const NO_DELTAS: &[Vec<u8>] = &[];

    fn row(object_id: &str, epoch: u64, rev: i64, dver: i64, status: &str) -> DeltaRow {
        DeltaRow {
            object_id: object_id.to_owned(),
            name: format!("name-{object_id}"),
            epoch,
            snapshot: RowSnapshot {
                rev,
                dver,
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

    fn encode(rows: &[DeltaRow], scratch: &Path) -> Vec<u8> {
        delta::encode(rows, None, scratch).expect("encodes")
    }

    /// A delta carrying the publish its rows were derived under, which
    /// is how the manifest learns field sets.
    fn encode_declared(
        rows: &[DeltaRow],
        spec: &actias_common::directory_spec::DirectorySpec,
        scratch: &Path,
    ) -> Vec<u8> {
        delta::encode(rows, Some(spec), scratch).expect("encodes")
    }

    fn statuses(merged: &Merged, scratch: &Path) -> Vec<(String, String, bool)> {
        delta::read(&merged.bytes, scratch)
            .expect("reads")
            .0
            .into_iter()
            .map(|row| {
                let status = row
                    .snapshot
                    .fields
                    .iter()
                    .find(|pair| pair.field == "status")
                    .map(|pair| pair.value.clone())
                    .unwrap_or_default();
                (row.object_id, status, row.tombstone)
            })
            .collect()
    }

    #[test]
    fn the_newest_row_per_object_wins() {
        let dir = tempfile::tempdir().expect("tempdir");
        let scratch = dir.path();
        let base = encode(
            &[row("a", 5, 1, 0, "old"), row("b", 5, 1, 0, "b1")],
            scratch,
        );
        let deltas = vec![
            encode(&[row("a", 5, 2, 0, "new")], scratch),
            encode(&[row("c", 5, 1, 0, "c1")], scratch),
        ];

        let merged = merge(Some(&base), &deltas, &Manifest::default(), scratch).expect("merges");
        assert_eq!(merged.rows, 3);
        assert_eq!(
            statuses(&merged, scratch),
            vec![
                ("a".to_owned(), "new".to_owned(), false),
                ("b".to_owned(), "b1".to_owned(), false),
                ("c".to_owned(), "c1".to_owned(), false),
            ]
        );
    }

    #[test]
    fn a_duplicated_delta_merges_to_the_same_result() {
        let dir = tempfile::tempdir().expect("tempdir");
        let scratch = dir.path();
        let delta = encode(&[row("a", 5, 2, 0, "new")], scratch);
        let base = encode(&[row("a", 5, 1, 0, "old")], scratch);

        let once = merge(
            Some(&base),
            std::slice::from_ref(&delta),
            &Manifest::default(),
            scratch,
        )
        .expect("merges");
        // Deltas are an unordered bag and a retry can deliver one
        // twice; the merge has to be idempotent or a resurrected file
        // would corrupt a class.
        let twice = merge(
            Some(&base),
            &[delta.clone(), delta],
            &Manifest::default(),
            scratch,
        )
        .expect("merges");
        assert_eq!(once.bytes, twice.bytes);
    }

    #[test]
    fn an_older_epoch_loses_however_high_its_rev() {
        let dir = tempfile::tempdir().expect("tempdir");
        let scratch = dir.path();
        let base = encode(&[row("a", 9, 2, 0, "current")], scratch);
        // A zombie ex-holder's late delta: higher rev, dead epoch.
        let stale = encode(&[row("a", 8, 99, 0, "stale")], scratch);

        let merged = merge(Some(&base), &[stale], &Manifest::default(), scratch).expect("merges");
        assert_eq!(statuses(&merged, scratch)[0].1, "current");
    }

    #[test]
    fn a_tombstone_retires_a_row_and_is_kept() {
        let dir = tempfile::tempdir().expect("tempdir");
        let scratch = dir.path();
        let base = encode(&[row("a", 5, 7, 0, "open")], scratch);
        let mut dead = row("a", 5, 0, 0, "");
        dead.tombstone = true;
        dead.snapshot = RowSnapshot::default();

        let merged = merge(
            Some(&base),
            &[encode(&[dead], scratch)],
            &Manifest::default(),
            scratch,
        )
        .expect("merges");
        assert_eq!(merged.rows, 0, "a tombstone is not a live row");
        let read = statuses(&merged, scratch);
        assert_eq!(read.len(), 1);
        assert!(
            read[0].2,
            "but it stays, or a late delta would resurrect it"
        );

        // Recreation at a higher epoch outranks the gravestone.
        let reborn = encode(&[row("a", 6, 1, 0, "open")], scratch);
        let after = merge(
            Some(&merged.bytes),
            &[reborn],
            &Manifest::default(),
            scratch,
        )
        .expect("merges");
        assert_eq!(after.rows, 1);
        assert!(!statuses(&after, scratch)[0].2);
    }

    #[test]
    fn the_floor_is_the_lowest_derived_row_and_ignores_tombstones_and_placeholders() {
        let dir = tempfile::tempdir().expect("tempdir");
        let scratch = dir.path();
        let mut dead = row("z", 5, 0, 0, "");
        dead.tombstone = true;
        dead.snapshot = RowSnapshot::default();
        // An identity repair found with nothing ever derived: rev 0,
        // no fields, offered so the index knows it exists.
        let mut placeholder = row("p", 5, 0, 0, "");
        placeholder.snapshot = RowSnapshot::default();
        let base = encode(
            &[
                row("a", 5, 1, 3, "a"),
                row("b", 5, 1, 2, "b"),
                dead,
                placeholder,
            ],
            scratch,
        );
        let merged = merge(Some(&base), NO_DELTAS, &Manifest::default(), scratch).expect("merges");
        assert_eq!(
            merged.min_dver, 2,
            "the floor is the least-derived live row; a tombstone's or a \
             placeholder's zero would keep every field building forever"
        );
        assert_eq!(
            merged.rows, 3,
            "the placeholder is a live row: it is how the index says the object exists"
        );
        let expected = super::super::identity::contribution("a")
            ^ super::super::identity::contribution("b")
            ^ super::super::identity::contribution("p");
        assert_eq!(
            merged.identities, expected,
            "and it folds into the identity checksum, which is what closes the gate"
        );
    }

    #[test]
    fn declarations_ride_the_deltas_to_the_manifest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let scratch = dir.path();
        let mut with_tags = row("a", 5, 1, 1, "open");
        with_tags.snapshot.fields.push(Pair {
            field: "tags".to_owned(),
            kind: "array".to_owned(),
            value: "[]".to_owned(),
        });
        let spec = actias_common::directory_spec::DirectorySpec::new(
            1,
            vec![
                ("status".to_owned(), "string".to_owned()),
                ("tags".to_owned(), "array".to_owned()),
            ],
        );

        let merged = merge(
            None,
            &[encode_declared(
                &[with_tags, row("b", 5, 1, 1, "shut")],
                &spec,
                scratch,
            )],
            &Manifest::default(),
            scratch,
        )
        .expect("merges");
        // The merge carries the publish, it does not infer a field set
        // from the rows: a row omitting a field is a legal value state
        // (nil is absent), so rows cannot say whether a field was added
        // later or simply absent here.
        assert_eq!(merged.declarations, vec![spec.clone()]);

        let mut manifest = Manifest::default();
        assert!(manifest.observe_declaration(&merged.declarations[0]));
        assert_eq!(manifest.dver, 1);
        manifest.min_dver = merged.min_dver;
        assert!(manifest.is_built("status"));
        assert!(manifest.is_built("tags"));
        assert!(!manifest.observe_declaration(&merged.declarations[0]));
    }

    #[test]
    fn a_repair_delta_carries_no_declaration_and_disturbs_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let scratch = dir.path();
        // Repair copies rows out of shipping manifests without knowing
        // which publish derived them, so it declares nothing and the
        // manifest's field set is left exactly as it was.
        let merged = merge(
            None,
            &[encode(&[row("a", 5, 1, 1, "open")], scratch)],
            &Manifest::default(),
            scratch,
        )
        .expect("merges");
        assert!(merged.declarations.is_empty());
    }

    #[test]
    fn an_empty_class_leaves_nothing_building() {
        let dir = tempfile::tempdir().expect("tempdir");
        let scratch = dir.path();
        let mut manifest = Manifest::default();
        manifest.observe_declaration(&actias_common::directory_spec::DirectorySpec::new(
            1,
            vec![("status".to_owned(), "string".to_owned())],
        ));

        // No rows left to backfill, so the floor must not sit below the
        // field-set generation and strand a field as building forever.
        let merged = merge(None, NO_DELTAS, &manifest, scratch).expect("merges");
        assert_eq!(merged.rows, 0);
        assert_eq!(merged.min_dver, manifest.dver);

        manifest.min_dver = merged.min_dver;
        assert!(manifest.is_built("status"));
    }
}
