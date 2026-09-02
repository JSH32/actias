//! Rechecking a listing's candidates against freshly recomputed rows.
//!
//! A listing answers from the index, which is a superset: its rows are
//! as of each object's last settled write, so some no longer match. A
//! visit takes those candidates and re-evaluates the whole predicate
//! against a row recomputed from the object's own state, dropping the
//! ones that have since stopped matching.
//!
//! This cannot go through sql. The overlay holds the stale row; the
//! recomputed one exists only in memory and belongs to no table. So the
//! tree is evaluated directly here, and the two evaluators have to agree:
//! every rule below mirrors what `predicate::render` emits, and the
//! parity is what the tests at the bottom are for.
//!
//! One asymmetry runs through all of it. A recheck may drop a row that
//! no longer matches, which costs nothing: the caller was going to skip
//! it anyway. A recheck that wrongly drops a row manufactures a false
//! negative, which is the one failure the whole design refuses. So
//! anything the evaluator cannot decide is an error, never a `false`,
//! and an error keeps the candidate and flags it. The superset principle
//! outranks the verification promise.

use super::evaluate::Row;
use super::overlay::{Candidate, Entry};
use super::predicate::{Compare, Condition, Where};
use super::row::Pair;
use super::shape::Value;
use super::version::RowVersion;

/// What a recheck learned about one candidate.
#[derive(Debug)]
pub enum Recomputed {
    /// The row the object's own state produces now.
    Row(Row),
    /// The object no longer exists. Dropping it invents no false
    /// negative: a destroyed object matches nothing, which is why the
    /// tombstone that would have said so is only a space optimization.
    Gone,
    /// No copy could be reached, with why. The candidate survives.
    Unreachable(String),
}

/// One candidate after its recheck.
#[derive(Debug, PartialEq)]
pub struct Checked {
    pub entry: Entry,
    /// Set when the row could not be rechecked. The entry then carries
    /// the index's stale fields, which is worth saying out loud rather
    /// than passing off as verified.
    pub unverified: bool,
    /// Why verification did not happen; `None` when it did.
    pub reason: Option<String>,
}

/// Re-evaluates `where_` against every candidate.
///
/// `recompute` is the seam: the worker restores a scratch copy and runs
/// the class's `directory` function against it, while tests hand back
/// rows directly. Nothing here knows about s3, leases or residency,
/// which is the point.
pub fn recheck<F>(candidates: Vec<Entry>, where_: &Where, mut recompute: F) -> Vec<Checked>
where
    F: FnMut(&Entry) -> Recomputed,
{
    let mut kept = Vec::new();
    for candidate in candidates {
        match recompute(&candidate) {
            Recomputed::Gone => {}
            Recomputed::Unreachable(reason) => kept.push(Checked {
                entry: candidate,
                unverified: true,
                reason: Some(reason),
            }),
            Recomputed::Row(row) => match matches(where_, &row) {
                // Still matches: the caller gets the fresh row, not the
                // stale one it was found by.
                Ok(true) => kept.push(Checked {
                    entry: Entry {
                        name: candidate.name,
                        object_id: candidate.object_id,
                        fields: row,
                    },
                    unverified: false,
                    reason: None,
                }),
                Ok(false) => {}
                Err(reason) => kept.push(Checked {
                    entry: candidate,
                    unverified: true,
                    reason: Some(reason),
                }),
            },
        }
    }
    kept
}

/// Whether one recomputed row satisfies the tree.
///
/// # Errors
/// Anything undecidable: a value whose kind the operator cannot ask
/// about, an empty combinator, a comparison across value families. Each
/// keeps the candidate rather than dropping it.
pub fn matches(where_: &Where, row: &Row) -> Result<bool, String> {
    // An empty tree selects everything, the same as the sql side's
    // `1 = 1`: `Class:visit {}` is every instance.
    for condition in &where_.0 {
        if !holds(condition, row)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn field<'row>(row: &'row Row, name: &str) -> Option<&'row Value> {
    row.iter()
        .find(|(known, _)| known == name)
        .map(|(_, value)| value)
}

fn holds(condition: &Condition, row: &Row) -> Result<bool, String> {
    match condition {
        Condition::Compare {
            field: name,
            op,
            value,
        } => {
            // An absent field never compares true. This is the sql
            // side's `NULL op ?` yielding NULL, and it is why `exists`
            // is the only way to ask about absence.
            let Some(found) = field(row, name) else {
                return Ok(false);
            };
            compare(name, found, *op, value)
        }
        Condition::In {
            field: name,
            values,
        } => {
            let Some(found) = field(row, name) else {
                return Ok(false);
            };
            // An empty list matches nothing, honestly, exactly as the
            // rendered `1 = 0` does.
            for value in values {
                if compare(name, found, Compare::Eq, value)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        Condition::StartsWith {
            field: name,
            prefix,
        } => match field(row, name) {
            None => Ok(false),
            Some(Value::Text(text)) => Ok(text.starts_with(prefix)),
            Some(_) => Err(format!(
                "'{name}' is not text in the recomputed row, so a prefix cannot be checked"
            )),
        },
        Condition::Contains { field: name, value } => match field(row, name) {
            None => Ok(false),
            Some(Value::Array(members)) => {
                for member in members {
                    if compare(name, member, Compare::Eq, value)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Some(_) => Err(format!(
                "'{name}' is not an array in the recomputed row, so membership cannot be checked"
            )),
        },
        // Presence is always decidable, which is what makes it the
        // operator for absence.
        Condition::Exists {
            field: name,
            present,
        } => Ok(field(row, name).is_some() == *present),
        Condition::Any(branches) => {
            group(branches, name_of(condition))?;
            for branch in branches {
                if matches(branch, row)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        Condition::All(branches) => {
            group(branches, name_of(condition))?;
            for branch in branches {
                if !matches(branch, row)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        Condition::None(branches) => {
            group(branches, name_of(condition))?;
            for branch in branches {
                if matches(branch, row)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
    }
}

fn name_of(condition: &Condition) -> &'static str {
    match condition {
        Condition::Any(_) => "any",
        Condition::All(_) => "all",
        Condition::None(_) => "none",
        _ => "combinator",
    }
}

/// An empty combinator is a declaration mistake either side of the
/// wire; the sql builder refuses it too rather than guessing.
fn group(branches: &[Where], which: &str) -> Result<(), String> {
    if branches.is_empty() {
        return Err(format!(
            "an empty '{which}' matches nothing it could mean; remove it"
        ));
    }
    Ok(())
}

/// Orders two values the way the overlay's binding would.
///
/// Numbers cross-compare, and a boolean is the integer the column
/// stores. Anything else is a genuine kind change (a class that made
/// `tags` text where it was an array), which is exactly the case the
/// manifest treats as a new field, so refusing here keeps the candidate
/// until the backfill settles it.
fn compare(name: &str, left: &Value, op: Compare, right: &Value) -> Result<bool, String> {
    use std::cmp::Ordering;

    let ordering = match (left, right) {
        (Value::Text(left), Value::Text(right)) => left.cmp(right),
        (Value::Integer(left), Value::Integer(right)) => left.cmp(right),
        (Value::Bool(left), Value::Bool(right)) => left.cmp(right),
        (Value::Bool(left), Value::Integer(right)) => i64::from(*left).cmp(right),
        (Value::Integer(left), Value::Bool(right)) => left.cmp(&i64::from(*right)),
        (left, right) => {
            let (Some(left), Some(right)) = (as_number(left), as_number(right)) else {
                return Err(format!(
                    "'{name}' changed kind since it was indexed, so the comparison cannot be checked"
                ));
            };
            // A NaN orders against nothing. Saying so keeps the row
            // rather than dropping it on an answer nobody can defend.
            match left.partial_cmp(&right) {
                Some(ordering) => ordering,
                None => {
                    return Err(format!("'{name}' is not a number that can be ordered"));
                }
            }
        }
    };

    Ok(match op {
        Compare::Eq => ordering == Ordering::Equal,
        Compare::Ne => ordering != Ordering::Equal,
        Compare::Lt => ordering == Ordering::Less,
        Compare::Lte => ordering != Ordering::Greater,
        Compare::Gt => ordering == Ordering::Greater,
        Compare::Gte => ordering != Ordering::Less,
    })
}

fn as_number(value: &Value) -> Option<f64> {
    match value {
        Value::Integer(number) => Some(*number as f64),
        Value::Number(number) => Some(*number),
        _ => None,
    }
}

/// One served row and whether it survived verification.
///
/// A flagged entry is served with its flag rather than dropped: the
/// caller decides what an unprovable row is worth, and dropping it here
/// would manufacture the false negative the design refuses.
#[derive(Debug)]
pub struct Visited {
    pub entry: Entry,
    pub unverified: bool,
    /// Why it could not be verified; absent when it was.
    pub reason: Option<String>,
}

/// One page of a verified read.
#[derive(Debug)]
pub struct VisitedPage {
    pub entries: Vec<Visited>,
    /// Continues where this page's candidates ended: verification may
    /// drop rows, so a short page with a cursor is normal.
    pub cursor: Option<String>,
}

/// What one object's shipping manifest said about its row, as the
/// verified read consumes it. The worker maps a manifest fetch onto
/// this; tests hand it back directly, so the ladder itself never
/// touches a store.
#[derive(Debug)]
pub enum Settled {
    /// No manifest has ever shipped for the object.
    Missing,
    /// The manifest is a deletion marker: proof of nonexistence.
    Deleted,
    /// The manifest exists but carries no directory row.
    NoRow,
    /// The settled row, verbatim from the manifest.
    Row {
        version: RowVersion,
        pairs: Vec<Pair>,
        /// The `(rev, dver)` of the newest failed derivation, when one
        /// is outstanding past the good row.
        failed: Option<(i64, i64)>,
    },
}

/// The verified read's answer for one candidate.
#[derive(Debug, PartialEq)]
pub enum Verdict {
    /// Still matches; serve this entry (the indexed one when the index
    /// was already the settled truth, the fresher one otherwise).
    Verified(Entry),
    /// Provably gone or provably no longer matching.
    Dropped,
    /// Could not be verified; the indexed entry is kept and says so.
    Flagged { entry: Entry, reason: String },
    /// The settled row is only the last good one, because a newer
    /// derivation failed. Recomputable from a restored copy, which is
    /// the one case where the metadata ladder cannot answer and the
    /// scratch path can. Typed rather than a `Flagged` the caller has
    /// to recognise by its message.
    ///
    /// `failed_at` is the version of the failure, which is what a
    /// caller caches its recomputation under: a later write mints a new
    /// rev, so a stale entry can only ever miss, never be believed.
    Recompute { entry: Entry, failed_at: RowVersion },
}

/// The metadata ladder: verifies one candidate against what its
/// object's manifest says, cheapest conclusion first, restoring
/// nothing.
///
/// - Version equality is the common case and the whole point: the
///   overlay's row IS the settled row (same `(epoch, rev, dver)` means
///   same derivation of the same state), and the sql predicate already
///   evaluated it, so equality alone verifies.
/// - A newer settled row is rechecked here in memory via [`matches`],
///   and the fresh row is what gets served: the caller was found by a
///   stale row but reads the current one.
/// - dver lag needs no branch, deliberately. A query can only name
///   built fields (building ones are refused at translation), and a
///   field is built only once `min_dver` proves every row carries it,
///   so any row a query could have matched answers correctly for the
///   fields that query used.
/// - Undecidable is flagged, never dropped: the superset principle
///   outranks the verification promise, exactly as [`recheck`] rules.
pub fn against_manifest(candidate: Candidate, settled: &Settled, where_: &Where) -> Verdict {
    let Candidate { entry, version } = candidate;
    match settled {
        // A placeholder (rev 0: repair found the identity with nothing
        // ever derived) against an object that indeed has nothing
        // derived: the index and the object agree, and agreement is
        // what verification means. Flagging it would mark every
        // never-written object unverified on every verified read.
        Settled::Missing | Settled::NoRow if version.rev == 0 => Verdict::Verified(entry),
        Settled::Missing => Verdict::Flagged {
            entry,
            reason: "nothing has shipped for this object yet, so its row cannot be checked"
                .to_owned(),
        },
        Settled::Deleted => Verdict::Dropped,
        Settled::NoRow => Verdict::Flagged {
            entry,
            reason: "the object's settled state carries no directory row".to_owned(),
        },
        Settled::Row {
            version: settled_version,
            pairs,
            failed,
        } => {
            // A failure newer than the good row means the row is the
            // last good value, not the current state: the honest answer
            // is the stale row, flagged. The verified read's scratch
            // tail recomputes these from a restored copy.
            if let Some((failed_rev, failed_dver)) = failed {
                let failure = RowVersion {
                    epoch: settled_version.epoch,
                    rev: (*failed_rev).max(0) as u64,
                    dver: (*failed_dver).max(0) as u64,
                };
                if failure.supersedes(settled_version) {
                    // Neither the index nor the manifest knows what the
                    // object says now: both hold the last good row. A
                    // restored copy does, so this is the tail's case.
                    return Verdict::Recompute {
                        entry,
                        failed_at: failure,
                    };
                }
            }

            match settled_version.cmp(&version) {
                std::cmp::Ordering::Equal => Verdict::Verified(entry),
                std::cmp::Ordering::Greater => match super::row::decode_pairs(pairs) {
                    Ok(fresh) => match matches(where_, &fresh) {
                        Ok(true) => Verdict::Verified(Entry {
                            name: entry.name,
                            object_id: entry.object_id,
                            fields: fresh,
                        }),
                        Ok(false) => Verdict::Dropped,
                        Err(reason) => Verdict::Flagged { entry, reason },
                    },
                    Err(reason) => Verdict::Flagged { entry, reason },
                },
                // A backfill re-derives a row from the object's own
                // settled state and offers it at the class's newer
                // declaration, so the index legitimately reads ahead of
                // the manifest by dver alone while the object stays
                // quiet. Same epoch and same rev means the same settled
                // state, and the shape is the backfill's alone: a live
                // write bumps the rev (the unchanged-row skip requires
                // an equal dver, so a new one always writes), and a
                // rehome bumps the epoch. The row therefore describes
                // exactly the state this manifest names, and verifying
                // it is what keeps a class queryable after a field is
                // added rather than flagging every quiet object forever.
                std::cmp::Ordering::Less
                    if settled_version.epoch == version.epoch
                        && settled_version.rev == version.rev =>
                {
                    Verdict::Verified(entry)
                }
                // Ahead by epoch or by rev is a state the manifest does
                // not know about, which the design says cannot happen.
                // Saying so beats guessing.
                std::cmp::Ordering::Less => Verdict::Flagged {
                    entry,
                    reason: "the index is ahead of the object's settled state".to_owned(),
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pairs: &[(&str, Value)]) -> Row {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), value.clone()))
            .collect()
    }

    fn entry(name: &str) -> Entry {
        Entry {
            name: name.to_owned(),
            object_id: format!("id-{name}"),
            fields: row(&[("state", Value::Text("open".into()))]),
        }
    }

    fn eq(field: &str, value: Value) -> Where {
        Where(vec![Condition::Compare {
            field: field.to_owned(),
            op: Compare::Eq,
            value,
        }])
    }

    #[test]
    fn a_row_that_still_matches_is_kept_with_its_fresh_fields() {
        let where_ = eq("state", Value::Text("open".into()));
        let fresh = row(&[
            ("state", Value::Text("open".into())),
            ("high_bid", Value::Integer(320)),
        ]);
        let kept = recheck(vec![entry("lot-a")], &where_, |_| {
            Recomputed::Row(fresh.clone())
        });

        assert_eq!(kept.len(), 1);
        assert!(!kept[0].unverified);
        // The caller gets what the object says now, not the stale row
        // that led us to it.
        assert_eq!(kept[0].entry.fields, fresh);
    }

    #[test]
    fn a_row_that_stopped_matching_is_dropped() {
        let where_ = eq("state", Value::Text("open".into()));
        let kept = recheck(vec![entry("lot-a")], &where_, |_| {
            Recomputed::Row(row(&[("state", Value::Text("sold".into()))]))
        });
        assert!(kept.is_empty(), "this is the whole point of a visit");
    }

    #[test]
    fn an_unreachable_candidate_is_flagged_never_dropped() {
        let where_ = eq("state", Value::Text("open".into()));
        let kept = recheck(vec![entry("lot-a")], &where_, |_| {
            Recomputed::Unreachable("no reachable copy".to_owned())
        });

        // Dropping here would manufacture a false negative, which the
        // design refuses even at the cost of the verification promise.
        assert_eq!(kept.len(), 1);
        assert!(kept[0].unverified);
        assert_eq!(kept[0].reason.as_deref(), Some("no reachable copy"));
    }

    #[test]
    fn a_destroyed_object_is_skipped() {
        let where_ = eq("state", Value::Text("open".into()));
        let kept = recheck(vec![entry("lot-a")], &where_, |_| Recomputed::Gone);
        // Not a false negative: a destroyed object matches nothing.
        assert!(kept.is_empty());
    }

    #[test]
    fn an_absent_field_never_compares_true() {
        let fresh = row(&[("state", Value::Text("open".into()))]);
        for op in [Compare::Eq, Compare::Ne, Compare::Lt, Compare::Gt] {
            let where_ = Where(vec![Condition::Compare {
                field: "high_bid".to_owned(),
                op,
                value: Value::Integer(100),
            }]);
            assert_eq!(
                matches(&where_, &fresh),
                Ok(false),
                "absence is queried with exists, never with a comparison"
            );
        }
    }

    #[test]
    fn exists_is_how_absence_is_asked_about() {
        let fresh = row(&[("state", Value::Text("open".into()))]);
        let present = Where(vec![Condition::Exists {
            field: "state".to_owned(),
            present: true,
        }]);
        let absent = Where(vec![Condition::Exists {
            field: "reserve".to_owned(),
            present: false,
        }]);
        assert_eq!(matches(&present, &fresh), Ok(true));
        assert_eq!(matches(&absent, &fresh), Ok(true));
    }

    #[test]
    fn numbers_compare_as_numbers_across_integer_and_float() {
        let fresh = row(&[("high_bid", Value::Integer(320))]);
        let where_ = Where(vec![Condition::Compare {
            field: "high_bid".to_owned(),
            op: Compare::Gt,
            value: Value::Number(99.5),
        }]);
        // The overlay binds by kind so these meet as numbers there; the
        // in-memory side has to agree or a visit would drop rows the
        // listing found.
        assert_eq!(matches(&where_, &fresh), Ok(true));
    }

    #[test]
    fn a_field_that_changed_kind_is_undecidable_rather_than_false() {
        let fresh = row(&[("tags", Value::Text("vintage".into()))]);
        let where_ = Where(vec![Condition::Compare {
            field: "tags".to_owned(),
            op: Compare::Eq,
            value: Value::Integer(3),
        }]);
        // Answering `false` here would silently drop the object. The
        // error keeps it, flagged, until the backfill settles the kind.
        assert!(matches(&where_, &fresh).is_err());
    }

    #[test]
    fn an_undecidable_row_is_kept_and_flagged() {
        let where_ = eq("tags", Value::Integer(3));
        let kept = recheck(vec![entry("lot-a")], &where_, |_| {
            Recomputed::Row(row(&[("tags", Value::Text("vintage".into()))]))
        });
        assert_eq!(kept.len(), 1);
        assert!(kept[0].unverified);
    }

    #[test]
    fn contains_walks_an_array_field() {
        let fresh = row(&[(
            "tags",
            Value::Array(vec![
                Value::Text("vintage".into()),
                Value::Text("rare".into()),
            ]),
        )]);
        let hit = Where(vec![Condition::Contains {
            field: "tags".to_owned(),
            value: Value::Text("rare".into()),
        }]);
        let miss = Where(vec![Condition::Contains {
            field: "tags".to_owned(),
            value: Value::Text("mint".into()),
        }]);
        assert_eq!(matches(&hit, &fresh), Ok(true));
        assert_eq!(matches(&miss, &fresh), Ok(false));
    }

    #[test]
    fn in_matches_any_listed_value_and_an_empty_list_matches_nothing() {
        let fresh = row(&[("state", Value::Text("sold".into()))]);
        let hit = Where(vec![Condition::In {
            field: "state".to_owned(),
            values: vec![Value::Text("open".into()), Value::Text("sold".into())],
        }]);
        let empty = Where(vec![Condition::In {
            field: "state".to_owned(),
            values: vec![],
        }]);
        assert_eq!(matches(&hit, &fresh), Ok(true));
        assert_eq!(matches(&empty, &fresh), Ok(false));
    }

    #[test]
    fn combinators_nest_the_way_the_sql_side_groups_them() {
        let fresh = row(&[
            ("state", Value::Text("sold".into())),
            ("high_bid", Value::Integer(600)),
        ]);
        let where_ = Where(vec![Condition::Any(vec![
            eq("state", Value::Text("open".into())),
            Where(vec![
                Condition::Compare {
                    field: "state".to_owned(),
                    op: Compare::Eq,
                    value: Value::Text("sold".into()),
                },
                Condition::Compare {
                    field: "high_bid".to_owned(),
                    op: Compare::Gte,
                    value: Value::Integer(500),
                },
            ]),
        ])]);
        assert_eq!(matches(&where_, &fresh), Ok(true));

        let none = Where(vec![Condition::None(vec![eq(
            "state",
            Value::Text("sold".into()),
        )])]);
        assert_eq!(matches(&none, &fresh), Ok(false));
    }

    #[test]
    fn an_empty_combinator_is_refused_on_both_sides_of_the_wire() {
        let fresh = row(&[("state", Value::Text("open".into()))]);
        for condition in [
            Condition::Any(vec![]),
            Condition::All(vec![]),
            Condition::None(vec![]),
        ] {
            assert!(matches(&Where(vec![condition]), &fresh).is_err());
        }
    }

    #[test]
    fn an_empty_tree_selects_everything() {
        let fresh = row(&[("state", Value::Text("open".into()))]);
        assert_eq!(matches(&Where::default(), &fresh), Ok(true));
    }

    fn candidate(epoch: u64, rev: u64, dver: u64) -> Candidate {
        Candidate {
            entry: Entry {
                name: "lot-a".to_owned(),
                object_id: "id-a".to_owned(),
                fields: row(&[("state", Value::Text("open".into()))]),
            },
            version: RowVersion { epoch, rev, dver },
        }
    }

    fn settled_row(epoch: u64, rev: u64, state: &str) -> Settled {
        Settled::Row {
            version: RowVersion {
                epoch,
                rev,
                dver: 0,
            },
            pairs: vec![Pair {
                field: "state".to_owned(),
                kind: "string".to_owned(),
                value: state.to_owned(),
            }],
            failed: None,
        }
    }

    #[test]
    fn a_placeholder_agrees_with_an_object_that_never_derived() {
        let where_ = eq("state", Value::Text("open".into()));
        // rev 0 is repair's "exists, never derived"; nothing shipped,
        // or shipped with no row, says the same thing from the other
        // side, so the pair verifies rather than flags.
        for settled in [Settled::Missing, Settled::NoRow] {
            assert!(matches!(
                against_manifest(candidate(0, 0, 0), &settled, &where_),
                Verdict::Verified(_)
            ));
        }
        // A real indexed row against nothing settled is still the
        // undecidable case: kept, and said so.
        assert!(matches!(
            against_manifest(candidate(1, 1, 1), &Settled::Missing, &where_),
            Verdict::Flagged { .. }
        ));
    }
    #[test]
    fn version_equality_verifies_with_no_recheck_at_all() {
        let open = eq("state", Value::Text("open".into()));
        // Same (epoch, rev, dver) means same derivation of the same
        // settled state: the sql predicate already evaluated this exact
        // row, so equality alone is the proof. This is the common case
        // and the reason a visit costs one metadata read, not a restore.
        let verdict = against_manifest(candidate(3, 7, 0), &settled_row(3, 7, "sold"), &open);
        assert!(matches!(verdict, Verdict::Verified(_)));
    }

    #[test]
    fn a_newer_settled_row_is_rechecked_and_served_fresh() {
        let open = eq("state", Value::Text("open".into()));

        let kept = against_manifest(candidate(3, 7, 0), &settled_row(3, 9, "open"), &open);
        let Verdict::Verified(entry) = kept else {
            panic!("a still-matching newer row verifies");
        };
        // The caller was found by the stale row but reads the fresh one.
        assert_eq!(entry.fields, row(&[("state", Value::Text("open".into()))]));

        let dropped = against_manifest(candidate(3, 7, 0), &settled_row(3, 9, "sold"), &open);
        assert_eq!(dropped, Verdict::Dropped, "this is what visit is FOR");
    }

    #[test]
    fn a_deletion_marker_drops_and_everything_else_flags() {
        let open = eq("state", Value::Text("open".into()));
        assert_eq!(
            against_manifest(candidate(3, 7, 0), &Settled::Deleted, &open),
            Verdict::Dropped,
            "a deletion marker is proof of nonexistence"
        );
        for said in [Settled::Missing, Settled::NoRow] {
            let verdict = against_manifest(candidate(3, 7, 0), &said, &open);
            assert!(
                matches!(verdict, Verdict::Flagged { .. }),
                "absence of proof keeps the row, flagged: superset over verification"
            );
        }
    }

    #[test]
    fn a_failure_newer_than_the_good_row_asks_for_recomputation() {
        let open = eq("state", Value::Text("open".into()));
        let said = Settled::Row {
            version: RowVersion {
                epoch: 3,
                rev: 7,
                dver: 0,
            },
            pairs: vec![Pair {
                field: "state".to_owned(),
                kind: "string".to_owned(),
                value: "open".to_owned(),
            }],
            failed: Some((8, 0)),
        };
        let verdict = against_manifest(candidate(3, 7, 0), &said, &open);
        assert!(
            matches!(verdict, Verdict::Recompute { .. }),
            "the row is the LAST GOOD value; only a restored copy knows the current one"
        );

        // An old failure, already superseded by a good row, is history.
        let healed = Settled::Row {
            version: RowVersion {
                epoch: 3,
                rev: 9,
                dver: 0,
            },
            pairs: vec![Pair {
                field: "state".to_owned(),
                kind: "string".to_owned(),
                value: "open".to_owned(),
            }],
            failed: Some((8, 0)),
        };
        let verdict = against_manifest(candidate(3, 9, 0), &healed, &open);
        assert!(matches!(verdict, Verdict::Verified(_)));
    }

    #[test]
    fn an_index_ahead_of_settled_state_is_flagged_not_trusted() {
        let open = eq("state", Value::Text("open".into()));
        let verdict = against_manifest(candidate(3, 9, 0), &settled_row(3, 7, "open"), &open);
        assert!(
            matches!(verdict, Verdict::Flagged { .. }),
            "rows are settle-gated, so a newer REV in the index is a bug \
             somewhere; flag, never guess"
        );

        // Ahead by epoch is the same story one dimension up: a
        // residency the manifest has never heard of.
        let rehomed = against_manifest(candidate(4, 7, 0), &settled_row(3, 7, "open"), &open);
        assert!(matches!(rehomed, Verdict::Flagged { .. }));
    }

    #[test]
    fn a_backfilled_row_verifies_against_the_state_it_came_from() {
        let open = eq("state", Value::Text("open".into()));

        // What a backfill leaves behind: the object has not written
        // since, so its manifest still carries the row at the old
        // declaration, while the index holds the same settled state
        // re-derived at the class's current one. Same epoch, same rev,
        // higher dver.
        let verdict = against_manifest(candidate(3, 7, 2), &settled_row(3, 7, "sold"), &open);
        assert!(
            matches!(verdict, Verdict::Verified(_)),
            "a row derived from THIS settled state is verified by it; \
             flagging it would mark every quiet object of every class \
             that ever gained a field"
        );
    }

    #[test]
    fn an_undecodable_settled_row_keeps_the_candidate() {
        let open = eq("state", Value::Text("open".into()));
        let said = Settled::Row {
            version: RowVersion {
                epoch: 3,
                rev: 9,
                dver: 0,
            },
            pairs: vec![Pair {
                field: "state".to_owned(),
                kind: "integer".to_owned(),
                value: "not-a-number".to_owned(),
            }],
            failed: None,
        };
        let verdict = against_manifest(candidate(3, 7, 0), &said, &open);
        assert!(matches!(verdict, Verdict::Flagged { .. }));
    }

    #[test]
    fn starts_with_needs_text_and_says_so() {
        let text = row(&[("name", Value::Text("lot-2099".into()))]);
        let number = row(&[("name", Value::Integer(2099))]);
        let where_ = Where(vec![Condition::StartsWith {
            field: "name".to_owned(),
            prefix: "lot-".to_owned(),
        }]);
        assert_eq!(matches(&where_, &text), Ok(true));
        assert!(matches(&where_, &number).is_err());
    }
}
