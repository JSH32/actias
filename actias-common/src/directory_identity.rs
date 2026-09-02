//! The identity checksum: what the index and the placement store
//! compare to decide whether a class is intact.
//!
//! Counting is not enough. A class missing one row while holding one
//! ghost has the same count as a healthy one, so the count invariant
//! reads it as fine and nothing ever looks closer. A checksum over the
//! identities themselves cannot cancel that way: the two differ, and
//! the difference is what opens the gate.
//!
//! The fold is XOR over a fixed slice of each object id, which is
//! already a blake3 hash of the identity. Both sides therefore agree
//! without sharing a hash function: the placement store takes the same
//! hex prefix in SQL, and the compactor takes it here. XOR because the
//! two sides visit their rows in different orders and must still
//! agree, and because removing an identity is the same operation as
//! adding it.

/// Hex characters of an object id the checksum reads. Object ids are
/// blake3 hex, so this is 60 bits of hash, which is what fits an SQL
/// `bigint` after the same slice on the store's side.
const PREFIX_CHARS: usize = 15;

/// One identity's contribution to its class's checksum. Zero for an id
/// that is not the hex the platform mints, which only happens for rows
/// predating the identity column: they contribute nothing on either
/// side rather than making the two disagree forever.
pub fn contribution(object_id: &str) -> i64 {
    if object_id.len() < PREFIX_CHARS {
        return 0;
    }
    i64::from_str_radix(&object_id[..PREFIX_CHARS], 16).unwrap_or(0)
}

/// The checksum of a set of identities, in any order.
pub fn checksum<'a>(object_ids: impl IntoIterator<Item = &'a str>) -> i64 {
    object_ids
        .into_iter()
        .fold(0, |sum, id| sum ^ contribution(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Three properties the whole gate rests on: order does not
    /// matter, removing an identity undoes adding it, and a set that
    /// differs by a swap does not read as equal the way its count
    /// does.
    #[test]
    fn the_fold_is_order_free_reversible_and_swap_sensitive() {
        let a = "4a4e19c3d7b123c9d699716b54e8b1127e13d7f5135c10f0ccbd2d4ec2f1a163";
        let b = "18f9afd487df8a82e6dbe8ca930fef6fa5e431e422305ec2623cd6c9d44dd3f6";
        let c = "98631e9a7490b580a26dcdeb18793fff77432272eb5eda36887bf8e4716f7b26";

        assert_eq!(checksum([a, b, c]), checksum([c, a, b]));
        assert_eq!(checksum([a, b, c]) ^ contribution(c), checksum([a, b]));
        assert_ne!(
            checksum([a, b]),
            checksum([a, c]),
            "one missing identity and one ghost is exactly what a count \
             cannot see"
        );
        assert_eq!(
            checksum(std::iter::empty()),
            0,
            "an empty class folds to nothing"
        );
    }

    #[test]
    fn an_id_the_platform_did_not_mint_contributes_nothing() {
        assert_eq!(contribution(""), 0);
        assert_eq!(contribution("short"), 0);
        // Non-hex cannot be read as a number; it contributes nothing
        // rather than poisoning the class's checksum.
        assert_eq!(contribution("zzzzzzzzzzzzzzzz"), 0);
    }
}
