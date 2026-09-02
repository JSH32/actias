//! Rebuilding a class's rows from the objects' own shipping manifests.
//!
//! The failure this exists for: an object contributes its row on the
//! flight that settles a write, so an object that never writes again
//! never contributes one. Nothing in the write path can fix that, since
//! the write path is exactly what is not happening. A row that should be
//! in the index and is not is a false negative, the one failure the
//! whole design refuses, so there has to be a path that does not wait
//! for a write.
//!
//! That path is metadata only. Every shipping manifest carries the
//! object's settled directory row, so repair is a copy between two
//! pieces of metadata: read the manifests, take the rows, offer them.
//! No object file is opened, no lease is taken, nothing is woken. The
//! cost is one GET per object, not one restore per object, which is
//! what makes rebuilding a whole class affordable enough to be the
//! answer to "the index looks wrong" rather than a last resort.
//!
//! One case this cannot repair, deliberately: a manifest carrying no
//! row. That means either the class had no `directory` when the object
//! last shipped, or it has never derived one. Both call for the same
//! thing, deriving it on a future evaluation, and neither can be
//! answered from metadata because the row does not exist anywhere yet.
//!
//! What repair offers for those is the truth as it stands: an empty row
//! at rev 0. Offering nothing instead would leave the identity
//! invariant open forever, because the store knows the identity, the
//! index has no row for it, and only a write could ever add one: an
//! object that is created and only ever read would keep its class
//! reconciling on every pass. The empty row says "exists, never derived"
//! in the index's own vocabulary: it loses to any real derivation on
//! rev, carries no field a query could mistake for state, and takes no
//! part in the field-set floor. They are still counted, because a large
//! count is how an operator learns a backfill is what they need.

use super::delta::DeltaRow;
use super::row::RowSnapshot;

/// What one object's manifest says about its directory row.
#[derive(Debug, Clone)]
pub struct Carried {
    pub object_id: String,
    /// From the identity, not the manifest: a manifest is keyed by
    /// object id and does not carry the name a listing answers with.
    pub name: String,
    /// The lease epoch the manifest was written under. Repair offers
    /// rows at this epoch, never at one of its own choosing, so a
    /// repaired row ranks exactly where the shipped row would have and
    /// a live residency's newer write always wins.
    pub epoch: u64,
    /// The manifest is a deletion marker.
    pub deleted: bool,
    /// The settled row, absent when the object has never derived one.
    pub row: Option<RowSnapshot>,
}

/// What a repair pass produced.
#[derive(Debug, Default, PartialEq)]
pub struct Repaired {
    pub rows: Vec<DeltaRow>,
    /// Identities offered an empty row because nothing has ever derived
    /// one for them. Not an error: the object exists and has no state
    /// worth a field yet.
    pub without_row: usize,
    /// Deletion markers turned into tombstones.
    pub tombstones: usize,
}

/// Turns carried manifests into the rows a repair delta offers.
///
/// Offered, not written: these go through the same last-writer-wins
/// merge as any shipped row, on `(epoch, tombstone, rev, dver)`. So a
/// repair can never overwrite something newer, and running it twice
/// changes nothing the first run did not. That is what makes it safe to
/// run against a live class instead of during a maintenance window.
pub fn rows_from_manifests(carried: Vec<Carried>) -> Repaired {
    let mut repaired = Repaired::default();

    for object in carried {
        if object.deleted {
            // The tombstone lets compaction drop the row. Correctness
            // never depended on it landing, so this is reclaiming
            // space, not repairing a wrong answer.
            repaired.tombstones += 1;
            repaired.rows.push(DeltaRow {
                object_id: object.object_id,
                name: object.name,
                epoch: object.epoch,
                snapshot: RowSnapshot::default(),
                tombstone: true,
            });
            continue;
        }

        let snapshot = match object.row {
            Some(snapshot) => snapshot,
            None => {
                // The placeholder: rev 0 is what "never derived" means,
                // and the default snapshot is exactly that. Offered at
                // the manifest's epoch like any row (epoch 0 when the
                // object never shipped at all), so the object's first
                // real derivation outranks it on rev.
                repaired.without_row += 1;
                RowSnapshot::default()
            }
        };

        repaired.rows.push(DeltaRow {
            object_id: object.object_id,
            name: object.name,
            epoch: object.epoch,
            snapshot,
            tombstone: false,
        });
    }

    // Object-id order, so the same set of manifests always encodes to
    // the same bytes and a re-run of an interrupted repair is
    // content-identical to the run it replaces.
    repaired
        .rows
        .sort_by(|left, right| left.object_id.cmp(&right.object_id));
    repaired
}

/// One row the index currently holds.
#[derive(Debug, Clone)]
pub struct Indexed {
    pub object_id: String,
    pub name: String,
    /// The epoch the index holds this row at.
    pub epoch: u64,
}

/// Tombstones for rows whose objects no longer exist.
///
/// [`rows_from_manifests`] can only speak about objects it was handed a
/// manifest for, so it cannot notice an object that stopped existing:
/// there is nothing left to hand it. Two ways that happens, and only
/// one of them writes a tombstone on the way out.
///
/// `state:destroy()` runs inside a dispatch, so the write path offers
/// the tombstone itself. But an object with a declared lifespan can
/// simply expire: its claim lapses with nobody dispatching to it, and
/// no code runs on its behalf ever again. Nothing in the write path can
/// mark it, because the write path is exactly what stopped happening.
///
/// The row it left behind is a false positive, which is the survivable
/// direction: `visit` cannot verify an object that does not exist and
/// drops it. But `list` keeps answering with a ghost, the count
/// invariant reads a permanent mismatch against the placement store,
/// and the space is never reclaimed. So the reconciliation is against
/// the identities that still exist, not against manifests.
///
/// The tombstone goes in at the row's own epoch and needs no invented
/// one: the merge ranks `(epoch, tombstone, rev, dver)`, so at equal
/// epoch a tombstone already outranks the row it retires. That also
/// keeps reincarnation correct, because a name claimed again gets a
/// higher epoch and its first row beats this tombstone rather than
/// being buried by it.
pub fn tombstones_for_vanished(
    indexed: Vec<Indexed>,
    live: &std::collections::HashSet<String>,
) -> Vec<DeltaRow> {
    let mut rows: Vec<DeltaRow> = indexed
        .into_iter()
        .filter(|row| !live.contains(&row.object_id))
        .map(|row| DeltaRow {
            object_id: row.object_id,
            name: row.name,
            epoch: row.epoch,
            snapshot: RowSnapshot::default(),
            tombstone: true,
        })
        .collect();
    rows.sort_by(|left, right| left.object_id.cmp(&right.object_id));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directory::row::Pair;

    fn snapshot(rev: i64) -> RowSnapshot {
        RowSnapshot {
            rev,
            dver: 1,
            // Values ride encoded, the same spelling the manifest and
            // the deltas carry; repair copies them without decoding.
            fields: vec![Pair {
                field: "state".to_owned(),
                kind: "string".to_owned(),
                value: "open".to_owned(),
            }],
            failed: None,
        }
    }

    fn carried(id: &str, epoch: u64, row: Option<RowSnapshot>) -> Carried {
        Carried {
            object_id: id.to_owned(),
            name: format!("lot-{id}"),
            epoch,
            deleted: false,
            row,
        }
    }

    #[test]
    fn a_carried_row_is_offered_at_the_manifests_epoch() {
        let repaired = rows_from_manifests(vec![carried("a", 7, Some(snapshot(3)))]);

        assert_eq!(repaired.rows.len(), 1);
        let row = &repaired.rows[0];
        assert_eq!(row.name, "lot-a");
        // The epoch decides the merge, so taking it from the manifest
        // rather than inventing one is what keeps a repaired row from
        // beating a live residency's newer write.
        assert_eq!(row.epoch, 7);
        assert_eq!(row.snapshot.rev, 3);
        assert!(!row.tombstone);
    }

    #[test]
    fn an_object_that_never_derived_a_row_is_offered_an_empty_one() {
        let repaired = rows_from_manifests(vec![
            carried("a", 1, Some(snapshot(1))),
            carried("b", 1, None),
            // Never shipped at all: no manifest, so no epoch either.
            carried("c", 0, None),
        ]);
        assert_eq!(repaired.rows.len(), 3);
        // Metadata cannot conjure the fields, but it can say the
        // object exists: without a row the identity invariant would
        // stay open on every pass, since only a write could close it.
        assert_eq!(repaired.without_row, 2);
        let empty: Vec<&DeltaRow> = repaired
            .rows
            .iter()
            .filter(|row| row.snapshot.rev == 0)
            .collect();
        assert_eq!(empty.len(), 2);
        for row in &empty {
            assert!(row.snapshot.fields.is_empty(), "no field can be invented");
            assert!(!row.tombstone, "existing is the opposite of deleted");
        }
        // At the manifest's epoch, so the object's own first derivation
        // (rev 1 at that epoch) outranks it.
        assert_eq!(empty[0].epoch, 1);
        assert_eq!(empty[1].epoch, 0);
    }

    #[test]
    fn a_deletion_marker_becomes_a_tombstone() {
        let repaired = rows_from_manifests(vec![Carried {
            deleted: true,
            ..carried("a", 9, None)
        }]);

        assert_eq!(repaired.tombstones, 1);
        assert_eq!(repaired.rows.len(), 1);
        assert!(repaired.rows[0].tombstone);
        assert_eq!(repaired.rows[0].epoch, 9);
        assert_eq!(
            repaired.without_row, 0,
            "a deleted object is not an object missing a row"
        );
    }

    #[test]
    fn a_deleted_manifest_that_still_carries_a_row_is_still_a_tombstone() {
        let repaired = rows_from_manifests(vec![Carried {
            deleted: true,
            ..carried("a", 9, Some(snapshot(4)))
        }]);

        // Deletion outranks the row it used to have; offering the row
        // would resurrect it in the index.
        assert_eq!(repaired.rows.len(), 1);
        assert!(repaired.rows[0].tombstone);
        assert_eq!(repaired.tombstones, 1);
    }

    #[test]
    fn rows_come_back_in_object_id_order() {
        let repaired = rows_from_manifests(vec![
            carried("c", 1, Some(snapshot(1))),
            carried("a", 1, Some(snapshot(1))),
            carried("b", 1, Some(snapshot(1))),
        ]);

        let ids: Vec<&str> = repaired
            .rows
            .iter()
            .map(|row| row.object_id.as_str())
            .collect();
        // Determinism is the point: a re-run of an interrupted repair
        // encodes to the same bytes as the run it replaces.
        assert_eq!(ids, vec!["a", "b", "c"]);
    }

    #[test]
    fn an_empty_class_repairs_to_nothing() {
        assert_eq!(rows_from_manifests(Vec::new()), Repaired::default());
    }

    fn indexed(id: &str, epoch: u64) -> Indexed {
        Indexed {
            object_id: id.to_owned(),
            name: format!("lot-{id}"),
            epoch,
        }
    }

    fn live(ids: &[&str]) -> std::collections::HashSet<String> {
        ids.iter().map(|id| (*id).to_owned()).collect()
    }

    #[test]
    fn an_expired_object_is_tombstoned_out_of_the_index() {
        let rows = tombstones_for_vanished(vec![indexed("a", 3), indexed("b", 4)], &live(&["a"]));

        // `b` expired: its claim lapsed with nobody dispatching to it,
        // so no code ever ran to offer a tombstone on its behalf.
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].object_id, "b");
        assert!(rows[0].tombstone);
    }

    #[test]
    fn the_tombstone_sits_at_the_rows_own_epoch() {
        let rows = tombstones_for_vanished(vec![indexed("a", 6)], &live(&[]));

        // No epoch is invented: the merge ranks tombstone second, so at
        // equal epoch it already outranks the row it retires.
        assert_eq!(rows[0].epoch, 6);
    }

    #[test]
    fn a_reincarnation_outranks_the_tombstone_that_retired_it() {
        let tombstone = &tombstones_for_vanished(vec![indexed("a", 6)], &live(&[]))[0];
        let reclaimed = rows_from_manifests(vec![carried("a", 7, Some(snapshot(1)))]);

        // A name claimed again gets a higher epoch, so its first row
        // beats this tombstone instead of being buried by it. That is
        // the reincarnation defence holding through repair.
        assert!(reclaimed.rows[0].epoch > tombstone.epoch);
    }

    #[test]
    fn a_live_object_is_left_alone() {
        let rows =
            tombstones_for_vanished(vec![indexed("a", 1), indexed("b", 1)], &live(&["a", "b"]));
        assert!(rows.is_empty(), "reconciliation is not a reason to churn");
    }
}
