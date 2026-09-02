//! The merge order: which of two rows describing the same object
//! wins. Directory deltas are an unordered bag of content-addressed
//! files, so this comparison is the entire ordering story; nothing may
//! depend on file names or arrival order.

/// Version of one directory row. Last-writer-wins, compared epoch
/// first, then rev, then dver.
///
/// Field order is the contract: the derived [`Ord`] compares
/// lexicographically top to bottom, and the tests below pin each
/// precedence so a reorder cannot land silently.
///
/// - `epoch` is the object's placement epoch, from the lease claim.
///   It survives destruction (the tombstone commits with an epoch
///   bump, `node_registry.rs`), so a recreated name outranks its own
///   tombstone.
/// - `rev` counts directory evaluations within one epoch. It may
///   regress across epochs; epoch-first comparison is what heals a
///   failover's ghost revs.
/// - `dver` is the publish ordinal of the `directory` declaration,
///   never content-derived. It orders backfilled rows against
///   manifest-copied rows at the same settled state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RowVersion {
    pub epoch: u64,
    pub rev: u64,
    pub dver: u64,
}

impl RowVersion {
    /// Whether a row at this version replaces one at `other` during a
    /// merge. Equal versions carry equal rows (both derive from the
    /// same settled state), so keeping either is correct.
    pub fn supersedes(&self, other: &RowVersion) -> bool {
        self > other
    }
}

#[cfg(test)]
mod tests {
    use super::RowVersion;

    fn v(epoch: u64, rev: u64, dver: u64) -> RowVersion {
        RowVersion { epoch, rev, dver }
    }

    #[test]
    fn a_reborn_name_outranks_its_tombstone() {
        // Destruction tombstoned at epoch 9; the recreated object's
        // first claim is a later epoch and must win from rev 1.
        assert!(v(10, 1, 3).supersedes(&v(9, 5, 3)));
    }

    #[test]
    fn epoch_heals_ghost_revs_after_failover() {
        // The dead node reached rev 43 but only rev 42 settled. The
        // new holder re-emits 42 under its higher epoch and must win.
        assert!(v(6, 42, 3).supersedes(&v(5, 43, 3)));
    }

    #[test]
    fn rev_outranks_dver_within_an_epoch() {
        // A real new write (higher rev) beats any backfill of an
        // older state, whatever declaration version produced it.
        assert!(v(4, 2, 0).supersedes(&v(4, 1, 9)));
    }

    #[test]
    fn dver_orders_backfill_at_the_same_settled_state() {
        // Backfill re-derives the same (epoch, rev) under a newer
        // declaration; the newer shape wins without a new write.
        assert!(v(4, 7, 2).supersedes(&v(4, 7, 1)));
    }

    #[test]
    fn equal_versions_do_not_supersede() {
        // A duplicated delta re-merges the same row; keeping the
        // incumbent is correct and the merge stays idempotent.
        assert!(!v(4, 7, 2).supersedes(&v(4, 7, 2)));
    }
}
