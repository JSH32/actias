//! Reading sqlite's write-ahead log, for shipping (docs/WAL-SHIPPING.md).
//!
//! The shipper needs one answer from a live `-wal` file: how many bytes
//! from the start form a checksum-valid sequence of whole frames ending
//! at a commit. Everything up to that point is safe to ship while the
//! single writer keeps appending, because the WAL is append-only between
//! checkpoints and frames are immutable once written. A torn or corrupt
//! tail is simply not part of the answer.
//!
//! Segments cut from one WAL share its header and its cumulative
//! checksums, so a generation's segments concatenate back into exactly
//! the original file; restore replays the concatenation once. The salts
//! identify the WAL incarnation: a checkpoint that restarts the log
//! changes them, and the shipper treats that as a base rotation.
//!
//! Format: 32-byte header (magic, format, page size, checkpoint
//! sequence, two salts, header checksum), then frames of a 24-byte
//! header (page number, commit size, the two salts, cumulative
//! checksum) plus one page. The magic's low bit picks big- or
//! little-endian words for the checksum. All documented, all stable.

/// The header's magic for little-endian checksums.
const MAGIC_LE: u32 = 0x377f_0682;
/// The header's magic for big-endian checksums.
const MAGIC_BE: u32 = 0x377f_0683;

const WAL_HEADER: usize = 32;
const FRAME_HEADER: usize = 24;

/// What the reader concluded about a WAL file's committed prefix.
#[derive(Debug, PartialEq, Eq)]
pub struct CommittedPrefix {
    /// Bytes from the file start (header included) through the last
    /// commit frame; 0 when the file has a header but no commits yet.
    pub len: usize,
    /// Database page size the WAL was written with.
    pub page_size: u32,
    /// The WAL incarnation; a restart (post-checkpoint) changes these.
    pub salts: (u32, u32),
    /// Commit frames inside the prefix.
    pub commits: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum WalError {
    /// Too short to hold a header, or not a WAL at all.
    NotAWal,
    /// The header's own checksum failed: not safe to ship anything.
    BadHeader,
}

/// The checksum-valid committed prefix of `wal`. Frames past the last
/// commit, torn tails and checksum mismatches end the scan; they are
/// the writer's business, not the shipper's.
pub fn committed_prefix(wal: &[u8]) -> Result<CommittedPrefix, WalError> {
    if wal.len() < WAL_HEADER {
        return Err(WalError::NotAWal);
    }
    let magic = u32::from_be_bytes(wal[0..4].try_into().expect("4 bytes"));
    let big_endian = match magic {
        MAGIC_LE => false,
        MAGIC_BE => true,
        _ => return Err(WalError::NotAWal),
    };
    let word = |at: usize| -> u32 {
        let bytes: [u8; 4] = wal[at..at + 4].try_into().expect("4 bytes");
        // Header fields other than checksum words are big-endian.
        u32::from_be_bytes(bytes)
    };
    let page_size = word(8);
    if !(512..=65536).contains(&page_size) || !page_size.is_power_of_two() {
        return Err(WalError::NotAWal);
    }
    let salts = (word(16), word(20));

    let mut sum = Checksum::new(big_endian);
    sum.push(&wal[0..24]);
    if (sum.s1, sum.s2) != (word(24), word(28)) {
        return Err(WalError::BadHeader);
    }

    let frame_size = FRAME_HEADER + page_size as usize;
    let mut offset = WAL_HEADER;
    let mut committed = 0usize;
    let mut commits = 0usize;
    while offset + frame_size <= wal.len() {
        let frame = &wal[offset..offset + frame_size];
        let f = |at: usize| -> u32 {
            u32::from_be_bytes(frame[at..at + 4].try_into().expect("4 bytes"))
        };
        // A frame from another incarnation ends the valid region.
        if (f(8), f(12)) != salts {
            break;
        }
        let mut next = sum;
        next.push(&frame[0..8]);
        next.push(&frame[FRAME_HEADER..]);
        if (next.s1, next.s2) != (f(16), f(20)) {
            break;
        }
        sum = next;
        offset += frame_size;
        if f(4) != 0 {
            // A commit frame: everything through here is shippable.
            committed = offset;
            commits += 1;
        }
    }

    Ok(CommittedPrefix {
        len: committed,
        page_size,
        salts,
        commits,
    })
}

/// Sqlite's cumulative WAL checksum: 32-bit words in the magic's
/// endianness, folded pairwise.
#[derive(Clone, Copy)]
struct Checksum {
    s1: u32,
    s2: u32,
    big_endian: bool,
}

impl Checksum {
    fn new(big_endian: bool) -> Self {
        Self {
            s1: 0,
            s2: 0,
            big_endian,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        debug_assert!(bytes.len().is_multiple_of(8));
        for pair in bytes.chunks_exact(8) {
            let (a, b) = if self.big_endian {
                (
                    u32::from_be_bytes(pair[0..4].try_into().expect("4 bytes")),
                    u32::from_be_bytes(pair[4..8].try_into().expect("4 bytes")),
                )
            } else {
                (
                    u32::from_le_bytes(pair[0..4].try_into().expect("4 bytes")),
                    u32::from_le_bytes(pair[4..8].try_into().expect("4 bytes")),
                )
            };
            self.s1 = self.s1.wrapping_add(a).wrapping_add(self.s2);
            self.s2 = self.s2.wrapping_add(b).wrapping_add(self.s1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::SqliteStorage;
    use std::path::Path;

    /// A WAL-mode database with `batches` committed transactions of
    /// pseudo-random rows; returns the connection so the WAL stays
    /// un-checkpointed.
    fn write_batches(path: &Path, batches: usize, seed: u64) -> SqliteStorage {
        let mut storage = SqliteStorage::open(path).expect("opens");
        storage
            .exec("CREATE TABLE IF NOT EXISTS t (k INTEGER, v TEXT)", &[])
            .expect("schema");
        let mut state = seed.max(1);
        for batch in 0..batches {
            storage.begin().expect("begins");
            let rows = 1 + (state % 5) as usize;
            for _ in 0..rows {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                storage
                    .exec(
                        "INSERT INTO t VALUES (?, ?)",
                        &[
                            serde_json::json!(state % 1000),
                            serde_json::json!(format!("{}-{}", batch, state % 97)),
                        ],
                    )
                    .expect("row");
            }
            storage.commit().expect("commits");
        }
        storage
    }

    fn table_sum(path: &Path) -> (i64, i64) {
        let mut storage = SqliteStorage::open_read_only(path).expect("opens");
        let rows = storage
            .query("SELECT count(*) AS c, coalesce(sum(k), 0) AS s FROM t", &[])
            .expect("sums");
        (
            rows[0]["c"].as_i64().expect("count"),
            rows[0]["s"].as_i64().expect("sum"),
        )
    }

    #[test]
    fn the_committed_prefix_counts_every_batch_and_ends_at_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("a.db");
        let _keep = write_batches(&db, 7, 42);

        let wal = std::fs::read(db.with_extension("db-wal")).expect("wal bytes");
        let prefix = committed_prefix(&wal).expect("parses");

        // Schema creation commits once, then the seven batches.
        assert_eq!(prefix.commits, 8, "one commit per transaction");
        assert_eq!(
            prefix.len,
            wal.len(),
            "a quiet WAL is committed through its end"
        );
    }

    #[test]
    fn a_torn_tail_and_a_corrupt_frame_are_not_part_of_the_answer() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("a.db");
        let _keep = write_batches(&db, 4, 7);

        let wal = std::fs::read(db.with_extension("db-wal")).expect("wal bytes");
        let whole = committed_prefix(&wal).expect("parses");

        // Torn tail: half a frame appended.
        let mut torn = wal.clone();
        torn.extend_from_slice(&wal[WAL_HEADER..WAL_HEADER + 100]);
        assert_eq!(committed_prefix(&torn).expect("parses"), whole);

        // A flipped byte in the last frame's page drops that frame (and,
        // it being the last commit, shortens the prefix).
        let mut corrupt = wal.clone();
        let last = corrupt.len() - 1;
        corrupt[last] ^= 0xff;
        let shorter = committed_prefix(&corrupt).expect("parses");
        assert!(shorter.len < whole.len);
        assert_eq!(shorter.commits, whole.commits - 1);
    }

    #[test]
    fn base_plus_concatenated_segments_replay_to_the_exact_database() {
        let dir = tempfile::tempdir().expect("tempdir");
        let live = dir.path().join("live.db");

        // Base: some history, checkpointed so the main file is the base.
        {
            let mut storage = write_batches(&live, 3, 99);
            storage.checkpoint().expect("folds");
        }
        let base = std::fs::read(&live).expect("base bytes");

        // More history, left in the WAL only.
        let keep = write_batches(&live, 6, 1234);
        let wal = std::fs::read(live.with_extension("db-wal")).expect("wal bytes");
        let prefix = committed_prefix(&wal).expect("parses");
        assert_eq!(prefix.len, wal.len());

        // Cut the committed region into segments at arbitrary commit
        // boundaries by re-scanning shorter prefixes, then restore from
        // base + concatenation and compare content exactly.
        let cut = {
            // A prefix of the WAL that ends at some interior commit.
            let mut best = 0;
            let frame = FRAME_HEADER + prefix.page_size as usize;
            let mut offset = WAL_HEADER;
            let mut seen = 0;
            while offset + frame <= prefix.len {
                offset += frame;
                let head = committed_prefix(&wal[..offset]).expect("parses");
                if head.len == offset && head.commits > seen {
                    seen = head.commits;
                    if head.commits == 3 {
                        best = offset;
                    }
                }
            }
            best
        };
        assert!(cut > 0, "an interior commit boundary exists");
        let segment_one = &wal[..cut];
        let segment_two = &wal[cut..prefix.len];

        let restored = dir.path().join("restored.db");
        std::fs::write(&restored, &base).expect("base lands");
        let mut concat = segment_one.to_vec();
        concat.extend_from_slice(segment_two);
        std::fs::write(restored.with_extension("db-wal"), concat).expect("wal lands");
        {
            let mut storage = SqliteStorage::open(&restored).expect("opens");
            storage.checkpoint().expect("replays");
        }

        drop(keep);
        assert_eq!(table_sum(&restored), table_sum(&live));
    }
}
