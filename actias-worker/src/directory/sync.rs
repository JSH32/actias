//! The directory syncer: settled rows leave the node as deltas.
//!
//! Sibling of the shipper, and deliberately a different shape. The
//! shipper is per object, because durability is per object. This is per
//! node, because a directory delta is a bag of rows and one file per
//! node per interval is cheaper than one per object: N objects writing
//! during an interval cost one upload per class, not N.
//!
//! Three properties the rest of the design leans on:
//!
//! - **Settled, not committed.** A row is offered here only after the
//!   flight carrying it wrote the object's manifest, so the index
//!   describes the durable universe and can never name a state a crash
//!   took back.
//! - **Coalesced by last-writer-wins.** Rows carry `(epoch, rev, dver)`
//!   and a later one simply replaces an earlier one for the same
//!   object, so a thousand writes in an interval collapse to one row
//!   with no loss of meaning. The shipper cannot do this; a WAL frame
//!   is not replaceable.
//! - **Content-addressed.** A delta is named by the hash of its bytes,
//!   so a retried upload writes the same name with the same content,
//!   two nodes can never collide, and nothing about correctness depends
//!   on node identity or on file names carrying order.
//!
//! Nothing here is on an acknowledgment path. A caller never waits for
//! a delta, and a lost one costs freshness the repair paths recover.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use actias_worker_core::directory::delta::{self, DeltaRow};
use actias_worker_core::directory::row::RowSnapshot;
use actias_worker_core::directory::version::RowVersion;

/// Uploads one delta: `(class, name, bytes)`. Boxed so tests can
/// capture instead of talking to a store, the way [`crate::shipper`]
/// boxes its flight.
pub type PutDeltaFn = Arc<
    dyn Fn(ClassKey, String, Vec<u8>) -> futures::future::BoxFuture<'static, Result<(), String>>
        + Send
        + Sync,
>;

/// The class a delta belongs to. Deltas live under the class's prefix,
/// so a node hosting several classes writes one delta per class.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClassKey {
    pub scope_id: String,
    pub class: String,
}

/// One object's row, waiting for the next flush.
#[derive(Clone, Debug)]
struct Pending {
    name: String,
    epoch: u64,
    snapshot: RowSnapshot,
    /// A destroyed object contributes a tombstone rather than a row;
    /// compaction merges it away once a later base exists.
    tombstone: bool,
}

impl Pending {
    /// How two offers for one object order. Epoch first, exactly as the
    /// merge order does, then **terminal beats live**: a tombstone
    /// carries no rev of its own, so ranking it by rev alone would let
    /// the row it retires outrank it. Destruction is final within its
    /// epoch, and a recreation always claims a higher one, so this
    /// cannot bury a reborn object.
    fn rank(&self) -> (u64, bool, u64, u64) {
        let version = RowVersion {
            epoch: self.epoch,
            rev: self.snapshot.rev.max(0) as u64,
            dver: self.snapshot.dver.max(0) as u64,
        };
        (version.epoch, self.tombstone, version.rev, version.dver)
    }
}

/// Node-wide pending directory rows, grouped by the class they belong
/// to.
pub struct DirectorySyncer {
    pending: Mutex<HashMap<ClassKey, HashMap<String, Pending>>>,
    /// The declared field set each class's rows were derived under,
    /// newest publish per class. It rides the delta so the compactor
    /// can fold it into the manifest: field sets come from publishes,
    /// never from scraping rows, because an absent field is a legal
    /// value and rows therefore cannot say whether a field is new or
    /// merely missing here.
    declarations: Mutex<HashMap<ClassKey, actias_common::directory_spec::DirectorySpec>>,
    /// Every class this node has ever offered a row for; the pending
    /// map cannot serve, because a flush empties it.
    known: Mutex<std::collections::BTreeSet<ClassKey>>,
    put: PutDeltaFn,
    /// Where delta files are built before upload; the node's own data
    /// directory, so a delta never lands outside it.
    scratch: std::path::PathBuf,
    gauges: Arc<crate::directory::gauges::DirectoryGauges>,
}

impl DirectorySyncer {
    pub fn new(
        put: PutDeltaFn,
        scratch: std::path::PathBuf,
        gauges: Arc<crate::directory::gauges::DirectoryGauges>,
    ) -> Arc<Self> {
        Arc::new(Self {
            pending: Mutex::new(HashMap::new()),
            declarations: Mutex::new(HashMap::new()),
            known: Mutex::new(std::collections::BTreeSet::new()),
            put,
            scratch,
            gauges,
        })
    }

    /// Offers one object's settled row. Called after the flight
    /// carrying it landed, never before: the epoch comes from the lease
    /// the flight shipped under, because the object's own file does not
    /// know it.
    ///
    /// Last-writer-wins on `(epoch, rev, dver)`, so a straggler from an
    /// older residency cannot overwrite a newer row, and a repeated
    /// offer of the same row is a no-op.
    pub fn record(
        &self,
        class: ClassKey,
        object_id: String,
        name: String,
        epoch: u64,
        snapshot: RowSnapshot,
        declaration: Option<actias_common::directory_spec::DirectorySpec>,
    ) {
        // Keep the newest publish seen for the class. Two revisions can
        // be resident at once mid-deploy, and the older one's rows are
        // still legitimate; the manifest merges by version either way,
        // so carrying the newest is what makes progress.
        if let Some(spec) = declaration {
            let mut held = lock(&self.declarations);
            match held.get(&class) {
                Some(current) if current.dver >= spec.dver => {}
                _ => {
                    held.insert(class.clone(), spec);
                }
            }
        }
        self.insert(
            class,
            object_id,
            Pending {
                name,
                epoch,
                snapshot,
                tombstone: false,
            },
        );
    }

    /// Offers a tombstone for a destroyed object. Correctness never
    /// depends on this landing: a verified read cannot hydrate an
    /// object that no longer exists and skips it, so the tombstone is
    /// a space optimization that lets compaction drop the row.
    pub fn record_destroyed(&self, class: ClassKey, object_id: String, name: String, epoch: u64) {
        self.insert(
            class,
            object_id,
            Pending {
                name,
                epoch,
                snapshot: RowSnapshot::default(),
                tombstone: true,
            },
        );
    }

    fn insert(&self, class: ClassKey, object_id: String, pending: Pending) {
        lock(&self.known).insert(class.clone());
        let mut map = lock(&self.pending);
        let rows = map.entry(class).or_default();
        // Equal ranks keep the incumbent, so a repeated offer of the
        // same row is a no-op rather than churn.
        if rows
            .get(&object_id)
            .is_some_and(|existing| existing.rank() >= pending.rank())
        {
            return;
        }
        rows.insert(object_id, pending);
    }

    /// Classes this node has written rows for. The compactor's work
    /// list: a node that never wrote a row for a class has no reason
    /// to fold it, and whichever node did will.
    ///
    /// Kept separately from the pending map, which empties on flush.
    pub fn known_classes(&self) -> Vec<ClassKey> {
        lock(&self.known).iter().cloned().collect()
    }

    /// Whether anything is waiting. A graceful drain flushes until this
    /// is true, the way the shipper drains to settled.
    pub fn settled(&self) -> bool {
        lock(&self.pending).values().all(|rows| rows.is_empty())
    }

    /// Writes one delta per class holding rows, and clears what it
    /// wrote. A class whose upload fails keeps its rows for the next
    /// flush rather than losing them, because a lost row is the one
    /// failure this design calls fatal.
    ///
    /// # Errors
    /// Returns the last upload failure's message. The rows a failed class
    /// held are kept for the next flush.
    pub async fn flush(&self) -> Result<(), String> {
        let drained: Vec<(ClassKey, HashMap<String, Pending>)> = {
            let mut map = lock(&self.pending);
            map.drain().filter(|(_, rows)| !rows.is_empty()).collect()
        };

        let mut failure = None;
        for (class, rows) in drained {
            let encoded: Vec<DeltaRow> = rows
                .iter()
                .map(|(object_id, pending)| DeltaRow {
                    object_id: object_id.clone(),
                    name: pending.name.clone(),
                    epoch: pending.epoch,
                    snapshot: pending.snapshot.clone(),
                    tombstone: pending.tombstone,
                })
                .collect();
            let declaration = lock(&self.declarations).get(&class).cloned();
            let bytes = match delta::encode(&encoded, declaration.as_ref(), &self.scratch) {
                Ok(bytes) => bytes,
                Err(error) => {
                    // Unencodable rows are dropped rather than retried
                    // forever: the next write re-derives them, and the
                    // repair paths cover an object that never writes.
                    actias_common::tracing::warn!(
                        class = %class.class,
                        %error,
                        "a directory delta could not be encoded"
                    );
                    continue;
                }
            };
            let name = format!("d-{}", blake3::hash(&bytes).to_hex());
            let carried = rows.len();
            if let Err(error) = (self.put)(class.clone(), name, bytes).await {
                self.gauges.count(&self.gauges.flush_failures);
                actias_common::tracing::warn!(
                    class = %class.class,
                    %error,
                    "a directory delta could not be uploaded; keeping its rows"
                );
                let mut map = lock(&self.pending);
                let held = map.entry(class).or_default();
                for (object_id, pending) in rows {
                    held.entry(object_id).or_insert(pending);
                }
                failure = Some(error);
            } else {
                self.gauges.count(&self.gauges.flushes);
                self.gauges.add(&self.gauges.flushed_rows, carried);
            }
        }

        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

/// A poisoned lock here means a panic while holding pending rows; the
/// rows are recoverable state, so proceeding beats bringing the node
/// down over an index.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use actias_worker_core::directory::row::Pair;

    type Uploads = Arc<Mutex<Vec<(ClassKey, String, Vec<u8>)>>>;

    fn syncer() -> (Arc<DirectorySyncer>, Uploads, tempfile::TempDir) {
        let uploads: Uploads = Arc::new(Mutex::new(Vec::new()));
        let sink = uploads.clone();
        let put: PutDeltaFn = Arc::new(move |class, name, bytes| {
            let sink = sink.clone();
            Box::pin(async move {
                sink.lock()
                    .expect("no other holder")
                    .push((class, name, bytes));
                Ok(())
            })
        });
        let scratch = tempfile::tempdir().expect("tempdir");
        let syncer = DirectorySyncer::new(put, scratch.path().to_path_buf(), Arc::default());
        (syncer, uploads, scratch)
    }

    fn class() -> ClassKey {
        ClassKey {
            scope_id: "project".to_owned(),
            class: "Auction".to_owned(),
        }
    }

    fn snapshot(rev: i64, status: &str) -> RowSnapshot {
        RowSnapshot {
            rev,
            dver: 0,
            fields: vec![Pair {
                field: "status".to_owned(),
                kind: "string".to_owned(),
                value: status.to_owned(),
            }],
            failed: None,
        }
    }

    /// Reads a delta back through the kernel's own reader, so these
    /// tests exercise scheduling and leave the format to delta.rs.
    fn rows_in(bytes: &[u8]) -> Vec<(String, i64, String, bool)> {
        let dir = tempfile::tempdir().expect("tempdir");
        delta::read(bytes, dir.path())
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
                (row.object_id, row.snapshot.rev, status, row.tombstone)
            })
            .collect()
    }

    #[tokio::test]
    async fn writes_in_one_interval_collapse_to_one_row() {
        let (syncer, uploads, _scratch) = syncer();
        for rev in 1..=50 {
            syncer.record(
                class(),
                "obj-a".to_owned(),
                "lot42".to_owned(),
                5,
                snapshot(rev, "open"),
                None,
            );
        }
        syncer.record(
            class(),
            "obj-b".to_owned(),
            "lot43".to_owned(),
            5,
            snapshot(1, "closed"),
            None,
        );

        syncer.flush().await.expect("flushes");
        assert!(syncer.settled());

        let uploads = uploads.lock().expect("no other holder");
        assert_eq!(uploads.len(), 1, "one delta per class, not per write");
        let rows = rows_in(&uploads[0].2);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], ("obj-a".to_owned(), 50, "open".to_owned(), false));
        assert_eq!(rows[1], ("obj-b".to_owned(), 1, "closed".to_owned(), false));
    }

    #[tokio::test]
    async fn a_straggler_from_an_older_residency_loses() {
        let (syncer, uploads, _scratch) = syncer();
        syncer.record(
            class(),
            "obj-a".to_owned(),
            "lot42".to_owned(),
            9,
            snapshot(2, "current"),
            None,
        );
        // A late offer from a dead residency: higher rev, older epoch.
        syncer.record(
            class(),
            "obj-a".to_owned(),
            "lot42".to_owned(),
            8,
            snapshot(99, "stale"),
            None,
        );

        syncer.flush().await.expect("flushes");
        let uploads = uploads.lock().expect("no other holder");
        let rows = rows_in(&uploads[0].2);
        assert_eq!(rows[0].2, "current", "epoch outranks rev");
    }

    #[tokio::test]
    async fn a_destroyed_object_tombstones_over_its_row() {
        let (syncer, uploads, _scratch) = syncer();
        syncer.record(
            class(),
            "obj-a".to_owned(),
            "lot42".to_owned(),
            5,
            snapshot(7, "open"),
            None,
        );
        // A tombstone carries no rev of its own, so ranking it by rev
        // alone would let the row it retires outrank it.
        syncer.record_destroyed(class(), "obj-a".to_owned(), "lot42".to_owned(), 5);

        syncer.flush().await.expect("flushes");
        {
            let uploads = uploads.lock().expect("no other holder");
            let rows = rows_in(&uploads[0].2);
            assert_eq!(rows.len(), 1);
            assert!(rows[0].3, "the tombstone replaced the row");
        }

        // Recreation claims a higher epoch (the placement store bumps it
        // with the tombstone), so the reborn object outranks its own
        // gravestone and becomes visible again.
        syncer.record(
            class(),
            "obj-a".to_owned(),
            "lot42".to_owned(),
            6,
            snapshot(1, "open"),
            None,
        );
        syncer.flush().await.expect("flushes");
        let uploads = uploads.lock().expect("no other holder");
        let rows = rows_in(&uploads[1].2);
        assert_eq!(rows[0].2, "open");
        assert!(!rows[0].3, "a reborn name is not a tombstone");
    }

    #[tokio::test]
    async fn the_same_rows_encode_to_the_same_name() {
        let (first, uploads_a, _a) = syncer();
        let (second, uploads_b, _b) = syncer();
        for syncer in [&first, &second] {
            syncer.record(
                class(),
                "obj-b".to_owned(),
                "lot43".to_owned(),
                5,
                snapshot(1, "closed"),
                None,
            );
            syncer.record(
                class(),
                "obj-a".to_owned(),
                "lot42".to_owned(),
                5,
                snapshot(2, "open"),
                None,
            );
        }
        first.flush().await.expect("flushes");
        second.flush().await.expect("flushes");

        let a = uploads_a.lock().expect("no other holder")[0].1.clone();
        let b = uploads_b.lock().expect("no other holder")[0].1.clone();
        assert_eq!(a, b, "content addressing cannot depend on insertion order");
    }

    #[tokio::test]
    async fn a_failed_upload_keeps_its_rows() {
        let put: PutDeltaFn =
            Arc::new(|_, _, _| Box::pin(async { Err("store is down".to_owned()) }));
        let scratch = tempfile::tempdir().expect("tempdir");
        let syncer = DirectorySyncer::new(put, scratch.path().to_path_buf(), Arc::default());
        syncer.record(
            class(),
            "obj-a".to_owned(),
            "lot42".to_owned(),
            5,
            snapshot(1, "open"),
            None,
        );

        assert!(syncer.flush().await.is_err());
        assert!(
            !syncer.settled(),
            "a lost row is the one failure this design calls fatal"
        );
    }
}
