//! Node-wide directory counters for `/_metrics`, one struct shared by
//! every loop the directory runs: the write-path flush, the compactor,
//! reconciliation, the crash sweep, the backfill, the verified read and
//! the query refusals. Every loop the directory runs is countable
//! here rather than only from a log tail.
//!
//! Hand-rolled atomics like the shipping gauges, and for the same
//! reason: a scrape is rare, an increment is on a hot path, and a
//! metrics framework would earn nothing here.
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

#[derive(Default)]
pub struct DirectoryGauges {
    /// Deltas written by the flush loop, and the rows they carried.
    pub flushes: AtomicU64,
    pub flushed_rows: AtomicU64,
    /// Flushes whose upload failed; their rows are kept and retried.
    pub flush_failures: AtomicU64,
    /// Bases laid by the compactor, and folds that failed.
    pub folds: AtomicU64,
    pub fold_failures: AtomicU64,
    /// Reconciliation passes this node began, the classes it checked
    /// the identity invariant for, and how many of those checks opened
    /// the gate. `gate_opened` climbing on a quiet cluster is the sign
    /// something keeps the index and the placement store disagreeing.
    pub passes: AtomicU64,
    pub gate_checks: AtomicU64,
    pub gate_opened: AtomicU64,
    /// Rebuilds run (reconciliation or the operator verb), the rows
    /// they offered, the placeholders among them (identities with
    /// nothing derived), and rebuilds that failed.
    pub rebuilds: AtomicU64,
    pub rebuilt_rows: AtomicU64,
    pub placeholder_rows: AtomicU64,
    pub rebuild_failures: AtomicU64,
    /// Crash-scoped sweeps that took a departure, and the rows they
    /// repaired.
    pub sweeps: AtomicU64,
    pub swept_rows: AtomicU64,
    /// Backfill: rows re-derived at the current declaration, objects
    /// skipped because they could not be rebuilt, and how many were
    /// still behind after the last pass on this node (a gauge, so a
    /// dashboard shows it draining rather than a total climbing).
    pub backfilled_rows: AtomicU64,
    pub backfill_skipped: AtomicU64,
    pub backfill_remaining: AtomicI64,
    /// Overlays this node materialized (a base plus its unfolded deltas
    /// applied into a local file). The read side's cost: one per class
    /// per generation per node that answers for it, which is the number
    /// reader placement exists to bound.
    pub overlay_builds: AtomicU64,
    pub overlay_build_ms_total: AtomicU64,
    /// Deltas applied to an existing overlay in place rather than by a
    /// rebuild; the number that should climb on a hot class while
    /// `overlay_builds` does not.
    pub overlay_applies: AtomicU64,
    /// Queries this node forwarded to the class's reader, and queries
    /// it answered on behalf of another node.
    pub forwarded: AtomicU64,
    pub served_for_peer: AtomicU64,
    /// The verified read's ladder, by outcome.
    pub visit_verified: AtomicU64,
    pub visit_flagged: AtomicU64,
    pub visit_recomputed: AtomicU64,
    pub visit_dropped: AtomicU64,
    /// Queries refused by the kernel (an unknown field, a building
    /// field, a wrong operator family), per class. Per class because a
    /// refusal is usually one author's one class, and the count says
    /// which.
    refusals: Mutex<HashMap<String, u64>>,
}

impl DirectoryGauges {
    pub fn count(&self, counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub fn add(&self, counter: &AtomicU64, n: usize) {
        counter.fetch_add(n as u64, Ordering::Relaxed);
    }

    pub fn refused(&self, class: &str) {
        let mut refusals = self
            .refusals
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *refusals.entry(class.to_owned()).or_default() += 1;
    }

    /// A snapshot of the per-class refusals, in class order so the
    /// exposition is stable between scrapes.
    pub fn refusals(&self) -> Vec<(String, u64)> {
        let refusals = self
            .refusals
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut rows: Vec<(String, u64)> = refusals
            .iter()
            .map(|(class, n)| (class.clone(), *n))
            .collect();
        rows.sort();
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusals_count_per_class_and_list_in_order() {
        let gauges = DirectoryGauges::default();
        gauges.refused("Lot");
        gauges.refused("Account");
        gauges.refused("Lot");
        assert_eq!(
            gauges.refusals(),
            vec![("Account".to_owned(), 1), ("Lot".to_owned(), 2)]
        );
    }
}
