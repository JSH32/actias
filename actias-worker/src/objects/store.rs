//! Snapshot shipping: an object's durable truth leaves the node. Every
//! residency lays a generation, the main file at a checkpoint, and
//! every flight after that ships only the WAL frames committed since
//! the last one, litestream's model natively; the generation rotates
//! when the WAL or the segment count grows past the thresholds. The
//! manifest carries the lease epoch as the fence, and restore-on-spawn
//! brings the object back wherever it is next resident, which makes
//! failover and rehoming the same code path.

use actias_worker_core::directory::manifest::Manifest as DirectoryManifest;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use actias_worker_core::proto::Bytes;

/// What the store remembers about one object's shipped state: the
/// generation directory `e{epoch}-b{base}` holding a `base.db` plus
/// zero or more `wal-{n:05}.seg` slices of one live WAL.
#[derive(Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default = "three")]
    pub version: u32,
    /// The shipper's lease epoch; a zombie ex-owner's uploads lose to any
    /// newer epoch here.
    pub epoch: u64,
    /// Unix ms of the ship, informational.
    pub shipped_at: i64,
    /// Base index within the epoch; bumps when the generation restarts.
    #[serde(default)]
    pub base: u64,
    /// WAL segments in the generation, `wal-00000 .. wal-{n-1}`.
    #[serde(default)]
    pub segments: u32,
    /// True when this manifest is a deletion marker: the identity was
    /// forgotten at this epoch, and the store holds no data for it. A
    /// zombie ex-holder's ship loses the fence to it, and a recreation
    /// at a higher epoch reads the store as empty.
    #[serde(default)]
    pub deleted: bool,
    /// The object's settled directory row, when its class derives one.
    /// Carried here so every directory repair path is a metadata copy:
    /// the crash sweep and a full rebuild read manifests and never open
    /// an object's file.
    ///
    /// Additive and defaulted, so it needs no version ladder: a
    /// manifest written before this field reads as [`None`], which is
    /// exactly true of anything shipped before its class had a
    /// directory. Absent also means "no row yet", so the two cases
    /// deliberately look the same, because they call for the same
    /// thing: derive it on the next write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<actias_worker_core::directory::row::RowSnapshot>,
    /// The replica nodes the owner fans this generation's WAL out to,
    /// so a takeover knows whom to ask. Additive and defaulted: a
    /// manifest without it reads as "no replicas", which is true.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replicas: Vec<String>,
    /// Bytes of the generation's WAL the store's segments cover, so a
    /// takeover can tell whether a replica's copy is at least as long.
    #[serde(default)]
    pub wal_len: u64,
    /// The base file's length, and its content as an ordered list of
    /// chunk hashes: one blake3 hex per [`CHUNK_BYTES`] of the file,
    /// each stored under `objects/{id}/chunks/{hash}`. A rotation
    /// rewrites only the entries the folded frames dirtied.
    #[serde(default)]
    pub base_len: u64,
    #[serde(default)]
    pub chunks: Vec<String>,
}

fn three() -> u32 {
    3
}

/// The manifest layout this store reads and writes. Nothing older is
/// read: a bucket written before chunked bases is wiped, not migrated.
pub const MANIFEST_VERSION: u32 = 3;

/// A base is stored and described in ranges this long. A constant, not
/// a knob: changing it would change every hash ever written.
pub const CHUNK_BYTES: u64 = 1 << 20;

/// The name of a chunk list, for a replica to check a delta against:
/// blake3 over the hashes joined by newlines.
pub fn list_hash(chunks: &[String]) -> String {
    let mut hasher = blake3::Hasher::new();
    for hash in chunks {
        hasher.update(hash.as_bytes());
        hasher.update(b"\n");
    }
    hasher.finalize().to_hex().to_string()
}

/// How many chunks a base of `len` bytes has.
pub fn chunk_count(len: u64) -> usize {
    len.div_ceil(CHUNK_BYTES) as usize
}

/// The directory row a flight carries, if any.
type DirectoryRow = Option<actias_worker_core::directory::row::RowSnapshot>;

/// Reads the object's settled directory row for the manifest. A
/// failure is logged and dropped rather than failing the flight: the
/// row is an index over state that is already shipping, so losing it
/// costs freshness the repair paths recover, while failing the flight
/// would cost durability.
fn read_directory_row(storage: &mut actias_worker_core::storage::SqliteStorage) -> DirectoryRow {
    match actias_worker_core::directory::row::snapshot(storage) {
        Ok(row) => row,
        Err(error) => {
            actias_common::tracing::warn!(%error, "the directory row could not be read for shipping");
            None
        }
    }
}

/// Per-residency shipping state, owned by the object's ship closure. A
/// fresh residency has a fresh epoch, so it always starts by laying a
/// new generation; nothing needs recovering from the store.
#[derive(Default)]
pub struct ShipState {
    generation: Option<Generation>,
    /// The membership generation the fence was last checked under. The
    /// manifest is read once per residency, and again only after this
    /// node re-registered, because that is the one event under which
    /// another node can have taken the lease: a lease lives exactly as
    /// long as its holder's registration. A per-flight read narrowed the
    /// same race by one round trip and never closed it; the conditional
    /// write that would close it is the replica fence's job.
    fence_seen: Option<u64>,
    /// Whether a manifest of this residency has named its replicas. A
    /// takeover learns whom to ask from the manifest, so the first
    /// flight of a residency releases at the manifest like a flight
    /// with no replicas; from the second on, the quorum may answer.
    replicas_named: bool,
    /// The generation and WAL length the last released flight reached:
    /// what callers have been told is durable. A replica serving a read
    /// waits for its copy to reach this.
    released: Option<(u64, u64, u64)>,
    /// Set while a generation start has checkpointed the file but not
    /// landed its base and manifest: the frames are in the main file
    /// and in no manifest, so the next flight must lay that generation
    /// whatever the WAL looks like. Cleared when a manifest lands.
    must_rotate: Option<u64>,
    watermark: Arc<Watermark>,
}

/// What a residency has answered its callers with, published outside
/// the shipping lock so a replica's read can ask mid-flight. It carries
/// the residency's lease epoch from the start, so a reader can tell a
/// copy from an older residency apart before anything has been released
/// in this one.
#[derive(Default)]
pub struct Watermark {
    epoch: std::sync::atomic::AtomicU64,
    released: std::sync::Mutex<Option<(u64, u64, u64)>>,
}

impl Watermark {
    pub fn epoch(&self) -> u64 {
        self.epoch.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn released(&self) -> Option<(u64, u64, u64)> {
        self.released.lock().ok().and_then(|r| *r)
    }

    fn set(&self, released: (u64, u64, u64)) {
        if let Ok(mut slot) = self.released.lock() {
            *slot = Some(released);
        }
    }
}

impl ShipState {
    /// The state of a residency at `epoch`, before its first flight.
    pub fn for_epoch(epoch: u64) -> Self {
        let state = Self::default();
        state
            .watermark
            .epoch
            .store(epoch, std::sync::atomic::Ordering::Relaxed);
        state
    }

    /// The watermark readers ask, shared with the data plane.
    pub fn watermark(&self) -> Arc<Watermark> {
        self.watermark.clone()
    }
}

/// Operator-tunable shipping sizes, one copy per node.
#[derive(Clone, Copy)]
pub struct ShipThresholds {
    /// A WAL this large rotates the generation at the next flight; the
    /// floor under the fraction.
    pub rotate_bytes: u64,
    /// A WAL this fraction of the base's length rotates it too, so a
    /// large object's checkpoint folds a bounded share of the file.
    pub rotate_fraction: f64,
    /// So does this many segments, whatever their size.
    pub max_segments: u32,
}

impl ShipThresholds {
    /// The WAL length that rotates a generation over a base of
    /// `base_len` bytes.
    pub fn rotate_at(&self, base_len: u64) -> u64 {
        let by_fraction = (base_len as f64 * self.rotate_fraction) as u64;
        self.rotate_bytes.max(by_fraction)
    }
}

/// One fan-out: the generation it names, and either an append or a
/// generation lay.
pub struct FanoutRequest {
    pub epoch: u64,
    pub base: u64,
    /// Acks that answer the flight; the fan-out stops waiting on the
    /// rest soon after it has them. 0 waits for everyone.
    pub quorum: usize,
    pub payload: Payload,
}

pub enum Payload {
    /// The committed WAL prefix as a whole (so a replica behind can be
    /// resent from wherever it is), the offset the owner believes the
    /// replicas hold, and how far the store covers.
    Append {
        offset: u64,
        wal: Bytes,
        covered: u64,
    },
    /// A generation start: the new chunk list, the list it is a delta
    /// over (empty when every chunk goes), the base's length, and the
    /// chunk indices to stream from `file`. A replica not on `from_list`
    /// is sent every chunk instead.
    Lay {
        from_list: String,
        base_len: u64,
        chunks: Arc<Vec<String>>,
        dirty: Arc<Vec<u32>>,
        file: PathBuf,
    },
}

/// What a fan-out came back with.
pub struct FanoutOutcome {
    /// Replicas that hold everything through the prefix's end.
    pub acks: usize,
    /// A replica refused the epoch: another owner exists, and this
    /// flight must not land on the store either.
    pub fenced: bool,
}

/// Sends one fan-out to every replica; the worker builds it over the
/// data plane, tests over anything.
pub type FanoutFn = Arc<
    dyn Fn(
            FanoutRequest,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = FanoutOutcome> + Send>>
        + Send
        + Sync,
>;

/// Replication for one flight: where to fan out, how many acks release
/// the gate (0 is shadow mode: fan out, never release early), and the
/// release itself.
pub struct Replication {
    pub fanout: FanoutFn,
    pub node_ids: crate::objects::fanout::ReplicaSet,
    pub quorum: usize,
    pub release: std::sync::Mutex<Option<crate::objects::shipper::Release>>,
    pub gauges: Arc<crate::objects::replica::ReplicaGauges>,
    /// Set when a fan-out came back below the quorum: the replica fence
    /// could not have spoken, so the store's fence is re-read before the
    /// next put. Shared across a residency's flights.
    pub short: Arc<std::sync::atomic::AtomicBool>,
    /// Set when the fan-out replaced a dead replica: the next flight
    /// rotates, so the lay reaches the newcomer with the whole
    /// generation.
    pub relay: Arc<std::sync::atomic::AtomicBool>,
}

impl Replication {
    /// Fans out, releases on a quorum when `may_release`, and says
    /// whether the flight was fenced.
    async fn send(&self, request: FanoutRequest, may_release: bool) -> Result<(), String> {
        let started = std::time::Instant::now();
        let outcome = (self.fanout)(request).await;
        self.gauges
            .fanout_appends
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.gauges.fanout_ack_ms_total.fetch_add(
            started.elapsed().as_millis() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
        if outcome.fenced {
            return Err(FENCED_BY_REPLICAS.to_owned());
        }
        if may_release
            && self.quorum > 0
            && outcome.acks >= self.quorum
            && let Ok(mut release) = self.release.lock()
            && let Some(release) = release.take()
        {
            release.now();
            self.gauges
                .quorum_releases
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            actias_worker_core::drill::fault("after-quorum");
        }
        if outcome.acks < self.quorum {
            self.gauges
                .fanout_failures
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.short.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        Ok(())
    }
}

/// The error a flight fails with when a replica refused its epoch. The
/// shipper's retry re-reads the store's fence and stops; nothing is
/// rotated or re-sent under an epoch another node has outranked.
pub const FENCED_BY_REPLICAS: &str = "fenced by the replicas; another node owns the object";

/// The generation a residency is shipping into.
#[derive(Clone)]
struct Generation {
    base: u64,
    segments: u32,
    /// The WAL incarnation the offsets count against; learned from the
    /// first frames after the generation's checkpoint.
    salts: Option<(u32, u32)>,
    /// Shipped through this byte of the WAL file.
    offset: usize,
    /// The base as shipped: its length and chunk list, carried into
    /// every manifest of the generation and diffed at the next rotation.
    base_len: u64,
    chunks: Arc<Vec<String>>,
}

pub struct ObjectStore {
    client: aws_sdk_s3::Client,
    bucket: String,
    /// Directory bases and deltas, by their own key. Content-addressed
    /// and therefore immutable: a key can only ever name one byte
    /// string, so an entry is never stale and only byte pressure
    /// evicts it.
    ///
    /// Without it a hot class re-downloads its whole base on every
    /// compaction and on every overlay rebuild, on every node that
    /// queries it, so download volume grows with class size times
    /// compaction rate rather than with change.
    files: moka::future::Cache<String, Arc<[u8]>>,
    /// Directory file reads, and how many of them reached the store.
    /// Their difference is the cache doing its job; a fetch count that
    /// tracks the read count says the cache is too small for the
    /// classes this node serves.
    pub file_reads: std::sync::atomic::AtomicU64,
    pub file_fetches: std::sync::atomic::AtomicU64,
    /// Chunk puts and gets in flight at once per operation.
    parallel: usize,
    /// Chunks a rotation put, their bytes, and chunks a restore fetched
    /// from the store rather than the cache.
    pub chunk_puts: std::sync::atomic::AtomicU64,
    pub chunk_bytes_put: std::sync::atomic::AtomicU64,
    pub chunk_gets: std::sync::atomic::AtomicU64,
}

impl ObjectStore {
    pub fn new(
        client: aws_sdk_s3::Client,
        bucket: String,
        cache_bytes: u64,
        parallel: usize,
    ) -> Self {
        Self {
            client,
            bucket,
            files: moka::future::Cache::builder()
                .max_capacity(cache_bytes)
                .weigher(|_, bytes: &Arc<[u8]>| bytes.len().clamp(1, u32::MAX as usize) as u32)
                .build(),
            file_reads: std::sync::atomic::AtomicU64::new(0),
            file_fetches: std::sync::atomic::AtomicU64::new(0),
            parallel: parallel.max(1),
            chunk_puts: std::sync::atomic::AtomicU64::new(0),
            chunk_bytes_put: std::sync::atomic::AtomicU64::new(0),
            chunk_gets: std::sync::atomic::AtomicU64::new(0),
        }
    }

    fn manifest_key(object_id: &str) -> String {
        format!("objects/{object_id}/manifest.json")
    }

    fn chunk_key(object_id: &str, hash: &str) -> String {
        format!("objects/{object_id}/chunks/{hash}")
    }

    fn gen_key(object_id: &str, epoch: u64, base: u64) -> String {
        format!("objects/{object_id}/e{epoch}-b{base}/gen.json")
    }

    fn segment_key(object_id: &str, epoch: u64, base: u64, n: u32) -> String {
        format!("objects/{object_id}/e{epoch}-b{base}/wal-{n:05}.seg")
    }

    /// One flight: ship what changed since the last one, fenced. An
    /// existing manifest with a newer epoch means someone else owns the
    /// object now, and this upload is refused; the check is made once
    /// per residency and again after a re-registration, see
    /// [`ShipState`].
    ///
    /// The first flight lays a generation: a checkpoint, the main file
    /// as the base, every chunk of it. Every flight after ships the
    /// committed WAL frames since the last one as one segment, and the
    /// generation rotates when the WAL or the segment count grows past
    /// the thresholds; a rotation ships only the chunks the folded
    /// frames dirtied. A segment that cannot be cut (the WAL restarted
    /// behind the shipper) starts the next generation instead, which
    /// folds every frame in. A flight that lays a new generation sweeps
    /// old ones, keeping the newest previous as the rollback margin.
    // A flight names the residency (id, epoch, file), the node's sizes,
    // the membership its fence was checked under, and the replication
    // to fan out through; folding them into a struct would name nothing
    // the argument list does not.
    #[allow(clippy::too_many_arguments)]
    pub async fn ship(
        &self,
        object_id: &str,
        epoch: u64,
        file: &Path,
        state: &tokio::sync::Mutex<ShipState>,
        thresholds: ShipThresholds,
        membership: u64,
        replication: Option<&Replication>,
    ) -> Result<(), String> {
        let mut state = state.lock().await;
        // The fence is re-read when the membership moved, and when the
        // last fan-out fell short of its quorum, or there is no fan-out
        // at all: only a quorum of replicas can refuse a zombie, so a
        // flight without that answer asks the store before it puts.
        let replicas_short = replication.is_none_or(|r| {
            r.quorum == 0 || r.short.swap(false, std::sync::atomic::Ordering::Relaxed)
        });
        if state.fence_seen != Some(membership) || (replicas_short && state.fence_seen.is_some()) {
            if let Some(manifest) = self.manifest(object_id).await?
                && (manifest.epoch > epoch || (manifest.deleted && manifest.epoch >= epoch))
            {
                return Err(format!(
                    "fenced: epoch {epoch} lost to {}; this node no longer owns the object",
                    manifest.epoch
                ));
            }
            state.fence_seen = Some(membership);
        }

        let wal_len = wal_path(file).metadata().map(|m| m.len()).unwrap_or(0);
        let base_before = state.generation.as_ref().map(|g| g.base);
        let may_release = state.replicas_named;
        let chunks_before = state
            .generation
            .as_ref()
            .map(|g| g.chunks.clone())
            .unwrap_or_default();

        let relay =
            replication.is_some_and(|r| r.relay.swap(false, std::sync::atomic::Ordering::SeqCst));
        let next = match state.generation.as_ref() {
            None => {
                let base = state.must_rotate.unwrap_or(0);
                state.must_rotate = Some(base);
                self.start_generation(object_id, epoch, file, base, None, replication)
                    .await
            }
            Some(generation)
                if state.must_rotate.is_some()
                    || relay
                    || generation.segments >= thresholds.max_segments
                    || wal_len >= thresholds.rotate_at(generation.base_len) =>
            {
                // Rotation: the checkpoint folds every frame, shipped or
                // not, into the next base; nothing can be lost to the
                // boundary. A failed rotation keeps the generation as it
                // was, and `must_rotate` remembers that the file was
                // checkpointed, so the retry lays the generation rather
                // than reading an empty WAL as "nothing to ship". The
                // retry cannot know what the folded frames dirtied, so
                // it hashes everything (a generation with no salts).
                let base = state.must_rotate.unwrap_or(generation.base + 1);
                let prev = if state.must_rotate.is_some() {
                    Generation {
                        salts: None,
                        ..generation.clone()
                    }
                } else {
                    generation.clone()
                };
                state.must_rotate = Some(base);
                self.start_generation(object_id, epoch, file, base, Some(&prev), replication)
                    .await
            }
            Some(generation) => {
                let base = generation.base;
                let resumed = generation.clone();
                match self
                    .segment_flight(object_id, epoch, file, resumed, replication, may_release)
                    .await
                {
                    Ok(next) => Ok(next),
                    // A fence is final for this residency: no rotation,
                    // no resend, the store untouched.
                    Err(error) if error == FENCED_BY_REPLICAS => Err(error),
                    Err(error) => {
                        // A new generation folds in everything the
                        // segment could not carry.
                        actias_common::tracing::warn!(
                            object_id,
                            %error,
                            "segment flight failed; starting a generation"
                        );
                        let prev = generation.clone();
                        state.must_rotate = Some(base + 1);
                        self.start_generation(
                            object_id,
                            epoch,
                            file,
                            base + 1,
                            Some(&prev),
                            replication,
                        )
                        .await
                    }
                }
            }
        };
        let next = next?;
        state.must_rotate = None;
        state.generation = Some(next);

        // Every manifest of this residency names the replica set, so
        // from here a takeover can find them.
        state.replicas_named = replication.is_some();
        state.released = state
            .generation
            .as_ref()
            .map(|g| (epoch, g.base, g.offset as u64));
        if let Some(released) = state.released {
            state.watermark.set(released);
        }

        // A new generation retires old ones; failures only defer the
        // sweep to the next rotation.
        let base_after = state.generation.as_ref().map(|g| g.base);
        if base_after != base_before
            && let Some(current) = state.generation.as_ref()
            && let Err(error) = self
                .collect_garbage(
                    object_id,
                    (epoch, current.base),
                    &current.chunks,
                    &chunks_before,
                )
                .await
        {
            actias_common::tracing::warn!(object_id, %error, "generation sweep failed");
        }
        Ok(())
    }

    /// Deletes everything under the object except the current
    /// generation, the newest one before it (the rollback margin while
    /// the current one is young), the chunks either of them lists, and
    /// the manifest. `current_chunks` is the list just written and
    /// `previous_chunks` the one it replaced, so the margin's chunks are
    /// known without a read; any other kept generation's list is read
    /// from its `gen.json`.
    async fn collect_garbage(
        &self,
        object_id: &str,
        current: (u64, u64),
        current_chunks: &[String],
        previous_chunks: &[String],
    ) -> Result<(), String> {
        let prefix = format!("objects/{object_id}/");
        let mut generations: std::collections::BTreeMap<(u64, u64), Vec<String>> =
            std::collections::BTreeMap::new();
        let mut chunks: Vec<(String, String)> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let page = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(&prefix)
                .set_continuation_token(token)
                .send()
                .await
                .map_err(|e| e.into_service_error().to_string())?;
            for key in page.contents().iter().filter_map(|o| o.key()) {
                let Some(rest) = key.strip_prefix(&prefix) else {
                    continue;
                };
                if let Some(hash) = rest.strip_prefix("chunks/") {
                    chunks.push((hash.to_owned(), key.to_owned()));
                } else if let Some(generation) = rest
                    .split_once('/')
                    .and_then(|(dir, _)| parse_generation(dir))
                {
                    generations
                        .entry(generation)
                        .or_default()
                        .push(key.to_owned());
                }
                // manifest.json and anything unrecognized stay.
            }
            match page.next_continuation_token() {
                Some(next) if page.is_truncated() == Some(true) => token = Some(next.to_owned()),
                _ => break,
            }
        }

        generations.remove(&current);
        let margin = generations.keys().next_back().copied();
        if let Some(newest) = margin {
            generations.remove(&newest);
        }
        for key in generations.into_values().flatten() {
            self.delete(key).await?;
        }

        // Every chunk the two kept generations list stays; the margin's
        // list is the previous one when it was this residency's, and is
        // read from the store when it was not.
        let mut referenced: std::collections::HashSet<String> =
            current_chunks.iter().cloned().collect();
        if let Some((epoch, base)) = margin {
            if !previous_chunks.is_empty() {
                referenced.extend(previous_chunks.iter().cloned());
            } else if let Ok(bytes) = self.get(Self::gen_key(object_id, epoch, base)).await
                && let Ok(listed) = serde_json::from_slice::<GenerationRecord>(&bytes)
            {
                referenced.extend(listed.chunks);
            } else {
                // A margin whose list cannot be read keeps every chunk
                // for now; the next rotation retires it and sweeps.
                return Ok(());
            }
        }
        for (hash, key) in chunks {
            if !referenced.contains(&hash) {
                self.delete(key).await?;
            }
        }
        Ok(())
    }

    async fn delete(&self, key: String) -> Result<(), String> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map(|_| ())
            .map_err(|e| e.into_service_error().to_string())
    }

    /// Starts a generation: fold the WAL into the main file at a
    /// checkpoint, ship the file as chunks, and count frames from zero.
    /// With `prev` (the generation being rotated) only the chunks the
    /// folded frames dirtied are hashed and shipped; every other entry
    /// keeps its hash. Without it, or when the WAL restarted behind the
    /// shipper so frames were folded that nobody walked, every chunk is
    /// hashed. The raw read of the main file is safe exactly here: only
    /// the shipper checkpoints, and its flights never overlap, so
    /// between checkpoints the main file does not change.
    ///
    /// The order is chunks, replicas, manifest: a chunk is inert until a
    /// manifest lists it, so a zombie's chunk puts are garbage the next
    /// sweep collects, and a replica laid before the manifest holds a
    /// generation the manifest will name or the next flight relays.
    async fn start_generation(
        &self,
        object_id: &str,
        epoch: u64,
        file: &Path,
        base: u64,
        prev: Option<&Generation>,
        replication: Option<&Replication>,
    ) -> Result<Generation, String> {
        let source = file.to_path_buf();
        let prev_chunks = prev.map(|g| g.chunks.clone()).unwrap_or_default();
        let prev_salts = prev.and_then(|g| g.salts);
        let folded = tokio::task::spawn_blocking(move || -> Result<Folded, String> {
            fold_and_hash(&source, &prev_chunks, prev_salts)
        })
        .await
        .map_err(|e| e.to_string())??;
        let Folded {
            chunks,
            dirty,
            base_len,
            directory,
        } = folded;
        let chunks = Arc::new(chunks);
        let dirty = Arc::new(dirty);

        // The store gets the dirty chunks first, in parallel.
        {
            use futures::StreamExt;
            let indices: Vec<u32> = dirty.as_ref().clone();
            let list = chunks.clone();
            let puts = futures::stream::iter(indices)
                .map(move |index| {
                    let chunks = list.clone();
                    async move {
                        let bytes = read_chunk(file, index).await?;
                        self.chunk_puts
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        self.chunk_bytes_put
                            .fetch_add(bytes.len() as u64, std::sync::atomic::Ordering::Relaxed);
                        self.put(Self::chunk_key(object_id, &chunks[index as usize]), bytes)
                            .await
                    }
                })
                .buffer_unordered(self.parallel);
            futures::pin_mut!(puts);
            while let Some(outcome) = puts.next().await {
                outcome?;
            }
        }
        let record = GenerationRecord {
            base_len,
            chunks: chunks.as_ref().clone(),
        };
        self.put(
            Self::gen_key(object_id, epoch, base),
            serde_json::to_vec(&record).map_err(|e| e.to_string())?,
        )
        .await?;
        actias_worker_core::drill::fault("after-chunks");

        // The replicas next: a generation start releases at the
        // manifest, never on the quorum, since a takeover asks replicas
        // for the generation the manifest names, and a quorum holding a
        // generation no manifest names yet is a quorum nobody can find.
        if let Some(replication) = replication {
            replication
                .send(
                    FanoutRequest {
                        epoch,
                        base,
                        quorum: replication.quorum,
                        payload: Payload::Lay {
                            from_list: prev.map(|g| list_hash(&g.chunks)).unwrap_or_default(),
                            base_len,
                            chunks: chunks.clone(),
                            dirty: dirty.clone(),
                            file: file.to_path_buf(),
                        },
                    },
                    false,
                )
                .await?;
        }
        actias_worker_core::drill::fault("after-lay");
        self.put_manifest(
            object_id,
            &Manifest {
                version: MANIFEST_VERSION,
                epoch,
                shipped_at: actias_worker_core::extensions::objects::unix_now_ms(),
                base,
                segments: 0,
                deleted: false,
                directory,
                replicas: replication
                    .map(|r| r.node_ids.lock().unwrap_or_else(|p| p.into_inner()).clone())
                    .unwrap_or_default(),
                wal_len: 0,
                base_len,
                chunks: chunks.as_ref().clone(),
            },
        )
        .await?;
        Ok(Generation {
            base,
            segments: 0,
            salts: None,
            offset: 0,
            base_len,
            chunks,
        })
    }

    /// Ships the WAL frames committed since the last flight as one
    /// segment. Anything surprising (a restarted WAL, an unreadable
    /// prefix) is an error; the caller starts a new generation.
    #[allow(clippy::too_many_arguments)]
    async fn segment_flight(
        &self,
        object_id: &str,
        epoch: u64,
        file: &Path,
        generation: Generation,
        replication: Option<&Replication>,
        may_release: bool,
    ) -> Result<Generation, String> {
        let Generation {
            base,
            segments,
            salts,
            offset,
            base_len,
            chunks,
        } = generation;
        let wal = match tokio::fs::read(wal_path(file)).await {
            Ok(bytes) => bytes,
            // No WAL file means nothing committed since the checkpoint.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error.to_string()),
        };
        if wal.len() < 32 {
            // Truncated or empty: nothing new since the generation began.
            return Ok(Generation {
                base,
                segments,
                salts,
                offset,
                base_len,
                chunks,
            });
        }

        let prefix = actias_worker_core::wal::committed_prefix(&wal)
            .map_err(|e| format!("wal unreadable: {e:?}"))?;
        if let Some(expected) = salts
            && expected != prefix.salts
        {
            return Err("the wal restarted behind the shipper".to_owned());
        }
        if prefix.len <= offset {
            return Ok(Generation {
                base,
                segments,
                salts: salts.or(Some(prefix.salts)),
                offset,
                base_len,
                chunks,
            });
        }

        // Past the early returns, so frames really are being shipped:
        // the object wrote, and its row moved with it. Worth one open,
        // because flights coalesce to at most one per interval.
        let source = file.to_path_buf();
        let directory = tokio::task::spawn_blocking(move || {
            match actias_worker_core::storage::SqliteStorage::open(&source) {
                Ok(mut storage) => read_directory_row(&mut storage),
                Err(error) => {
                    actias_common::tracing::warn!(%error, "the directory row could not be read for shipping");
                    None
                }
            }
        })
        .await
        .unwrap_or(None);

        // Replicas first: their acks are what may answer the callers, and
        // a fence among them stops a zombie before it touches the store.
        let segment = wal[offset..prefix.len].to_vec();
        if let Some(replication) = replication {
            let mut prefix_bytes = wal;
            prefix_bytes.truncate(prefix.len);
            replication
                .send(
                    FanoutRequest {
                        epoch,
                        base,
                        quorum: replication.quorum,
                        payload: Payload::Append {
                            offset: offset as u64,
                            wal: Bytes::from(prefix_bytes),
                            covered: offset as u64,
                        },
                    },
                    may_release,
                )
                .await?;
        }
        self.put(Self::segment_key(object_id, epoch, base, segments), segment)
            .await?;
        actias_worker_core::drill::fault("after-segment");
        self.put_manifest(
            object_id,
            &Manifest {
                version: MANIFEST_VERSION,
                epoch,
                shipped_at: actias_worker_core::extensions::objects::unix_now_ms(),
                base,
                segments: segments + 1,
                deleted: false,
                directory,
                replicas: replication
                    .map(|r| r.node_ids.lock().unwrap_or_else(|p| p.into_inner()).clone())
                    .unwrap_or_default(),
                wal_len: prefix.len as u64,
                base_len,
                chunks: chunks.as_ref().clone(),
            },
        )
        .await?;
        actias_worker_core::drill::fault("after-manifest");
        Ok(Generation {
            base,
            segments: segments + 1,
            salts: Some(prefix.salts),
            offset: prefix.len,
            base_len,
            chunks,
        })
    }

    /// Writes the deletion marker at the bumped epoch and clears the
    /// store's data for the identity. The marker goes first: it is what
    /// fences a zombie's late ship, and a crash between the two steps
    /// leaves orphaned data behind a marker that already refuses it.
    ///
    /// # Errors
    /// Returns the store's message.
    pub async fn mark_deleted(&self, object_id: &str, epoch: u64) -> Result<(), String> {
        self.put_manifest(
            object_id,
            &Manifest {
                version: MANIFEST_VERSION,
                epoch,
                shipped_at: actias_worker_core::extensions::objects::unix_now_ms(),
                base: 0,
                segments: 0,
                deleted: true,
                // A forgotten identity has no row; the directory's own
                // tombstone is what removes it from the class.
                directory: None,
                replicas: Vec::new(),
                wal_len: 0,
                base_len: 0,
                chunks: Vec::new(),
            },
        )
        .await?;

        // Everything under the identity except the marker itself.
        let prefix = format!("objects/{object_id}/");
        let marker = Self::manifest_key(object_id);
        let mut token: Option<String> = None;
        loop {
            let page = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(&prefix)
                .set_continuation_token(token)
                .send()
                .await
                .map_err(|e| e.into_service_error().to_string())?;
            for key in page.contents().iter().filter_map(|o| o.key()) {
                if key == marker {
                    continue;
                }
                self.client
                    .delete_object()
                    .bucket(&self.bucket)
                    .key(key)
                    .send()
                    .await
                    .map_err(|e| e.into_service_error().to_string())?;
            }
            match page.next_continuation_token() {
                Some(next) if page.is_truncated() == Some(true) => token = Some(next.to_owned()),
                _ => break,
            }
        }
        Ok(())
    }

    /// Restores the last shipped state into `file`; false when nothing
    /// was ever shipped (a genuinely new object).
    ///
    /// # Errors
    /// Returns the store's message. An object that never shipped answers
    /// `false` rather than failing.
    pub async fn restore(&self, object_id: &str, file: &Path) -> Result<bool, String> {
        let Some(manifest) = self.manifest(object_id).await? else {
            return Ok(false);
        };

        // A deletion marker means the store remembers forgetting: any
        // local file is residue from a previous life and goes with the
        // sidecars, and the caller starts the identity fresh.
        if manifest.deleted {
            let _ = tokio::fs::remove_file(file).await;
            let _ = tokio::fs::remove_file(wal_path(file)).await;
            let _ = tokio::fs::remove_file(shm_path(file)).await;
            return Ok(false);
        }

        // A stale local WAL beside a restored base is poison: sqlite
        // would recover those old frames INTO the new file. The sidecars
        // go first, whatever path lays the base.
        let _ = tokio::fs::remove_file(wal_path(file)).await;
        let _ = tokio::fs::remove_file(shm_path(file)).await;

        self.lay_base(object_id, &manifest, file).await?;

        if manifest.segments > 0 {
            // Segments are consecutive slices of one WAL: concatenated
            // they ARE the original file, and sqlite replays it in one
            // pass at the checkpoint (proven in worker-core wal.rs).
            let mut wal = Vec::new();
            for n in 0..manifest.segments {
                wal.extend_from_slice(
                    &self
                        .get(Self::segment_key(
                            object_id,
                            manifest.epoch,
                            manifest.base,
                            n,
                        ))
                        .await?,
                );
            }
            tokio::fs::write(wal_path(file), wal)
                .await
                .map_err(|e| e.to_string())?;

            let target = file.to_path_buf();
            tokio::task::spawn_blocking(move || -> Result<(), String> {
                let mut storage = actias_worker_core::storage::SqliteStorage::open(&target)?;
                storage.checkpoint()
            })
            .await
            .map_err(|e| e.to_string())??;
        }
        Ok(true)
    }

    /// Lays the manifest's base into `file` from its chunks: the file is
    /// sized first, then every chunk is fetched (the cache first, the
    /// store on a miss) and written at its offset, `parallel` at a time.
    /// Nothing holds the file whole.
    async fn lay_base(
        &self,
        object_id: &str,
        manifest: &Manifest,
        file: &Path,
    ) -> Result<(), String> {
        use futures::StreamExt;
        if manifest.chunks.len() != chunk_count(manifest.base_len) {
            return Err(format!(
                "the manifest lists {} chunks for {} bytes",
                manifest.chunks.len(),
                manifest.base_len
            ));
        }
        let target = Arc::new(
            tokio::task::spawn_blocking({
                let file = file.to_path_buf();
                let len = manifest.base_len;
                move || -> Result<std::fs::File, String> {
                    let target = std::fs::OpenOptions::new()
                        .write(true)
                        .create(true)
                        .truncate(true)
                        .open(&file)
                        .map_err(|e| e.to_string())?;
                    target.set_len(len).map_err(|e| e.to_string())?;
                    Ok(target)
                }
            })
            .await
            .map_err(|e| e.to_string())??,
        );
        let listed: Vec<(usize, String)> = manifest.chunks.iter().cloned().enumerate().collect();
        let lays = futures::stream::iter(listed)
            .map(move |(index, hash)| {
                let target = target.clone();
                async move {
                    let bytes = self.chunk(object_id, &hash).await?;
                    tokio::task::spawn_blocking(move || -> Result<(), String> {
                        use std::os::unix::fs::FileExt;
                        target
                            .write_all_at(&bytes, index as u64 * CHUNK_BYTES)
                            .map_err(|e| e.to_string())
                    })
                    .await
                    .map_err(|e| e.to_string())?
                }
            })
            .buffer_unordered(self.parallel);
        futures::pin_mut!(lays);
        while let Some(outcome) = lays.next().await {
            outcome?;
        }
        Ok(())
    }

    /// One chunk by hash, through the content-addressed cache. A fetched
    /// chunk is hashed before it is trusted or cached.
    async fn chunk(&self, object_id: &str, hash: &str) -> Result<Arc<[u8]>, String> {
        let key = Self::chunk_key(object_id, hash);
        let expected = hash.to_owned();
        self.files
            .try_get_with(key.clone(), async {
                self.chunk_gets
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let bytes = self.get(key.clone()).await?;
                let actual = blake3::hash(&bytes).to_hex().to_string();
                if actual != expected {
                    return Err(format!("chunk {key} does not hash to its name"));
                }
                Ok::<_, String>(Arc::from(bytes))
            })
            .await
            .map_err(|error: Arc<String>| error.to_string())
    }

    pub async fn manifest(&self, object_id: &str) -> Result<Option<Manifest>, String> {
        let result = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(Self::manifest_key(object_id))
            .send()
            .await;

        match result {
            Ok(object) => {
                let bytes = object
                    .body
                    .collect()
                    .await
                    .map_err(|e| e.to_string())?
                    .into_bytes();
                let manifest: Manifest =
                    serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                if manifest.version != MANIFEST_VERSION {
                    return Err(format!(
                        "manifest version {} for {object_id}; this store reads version {MANIFEST_VERSION} only, and a bucket from before chunked bases is wiped, not migrated",
                        manifest.version
                    ));
                }
                Ok(Some(manifest))
            }
            Err(error) => {
                let service = error.into_service_error();
                if service.is_no_such_key() {
                    Ok(None)
                } else {
                    Err(service.to_string())
                }
            }
        }
    }

    async fn put(&self, key: String, bytes: Vec<u8>) -> Result<(), String> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(bytes.into())
            .send()
            .await
            .map(|_| ())
            .map_err(|e| e.into_service_error().to_string())
    }

    /// Uploads one directory delta under its class's prefix, named by
    /// its own content. Immutable by construction: the same rows encode
    /// to the same bytes and therefore the same key, so a retry after
    /// an ambiguous failure rewrites identical bytes and two nodes
    /// cannot collide.
    ///
    /// # Errors
    /// Returns the store's message.
    pub async fn put_directory_delta(
        &self,
        scope_id: &str,
        class: &str,
        name: &str,
        bytes: Vec<u8>,
    ) -> Result<(), String> {
        self.put(
            format!("directory/{scope_id}/{class}/deltas/{name}.sqlite"),
            bytes,
        )
        .await
    }

    fn directory_prefix(scope_id: &str, class: &str) -> String {
        format!("directory/{scope_id}/{class}/")
    }

    /// The class's directory manifest, or [`None`] before its first
    /// compaction. The only mutable key in the directory layout.
    ///
    /// # Errors
    /// Returns the store's message; a missing manifest is [`None`], not
    /// an error, because a class nobody has compacted yet is normal.
    pub async fn directory_manifest(
        &self,
        scope_id: &str,
        class: &str,
    ) -> Result<Option<DirectoryManifest>, String> {
        let key = format!("{}manifest.json", Self::directory_prefix(scope_id, class));
        match self.get(key).await {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|e| e.to_string()),
            Err(error) if error.contains("NoSuchKey") || error.contains("not found") => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Every delta currently under the class, by name. The reader's
    /// discovery step: nodes never announce a delta, they just write
    /// one, so listing the prefix is how anyone learns of it.
    ///
    /// # Errors
    /// Returns the store's message.
    pub async fn directory_deltas(
        &self,
        scope_id: &str,
        class: &str,
    ) -> Result<Vec<String>, String> {
        let prefix = format!("{}deltas/", Self::directory_prefix(scope_id, class));
        let mut names = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let page = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(&prefix)
                .set_continuation_token(token)
                .send()
                .await
                .map_err(|e| e.into_service_error().to_string())?;
            for key in page.contents().iter().filter_map(|object| object.key()) {
                if let Some(name) = key
                    .strip_prefix(&prefix)
                    .and_then(|rest| rest.strip_suffix(".sqlite"))
                {
                    names.push(name.to_owned());
                }
            }
            token = page.next_continuation_token().map(str::to_owned);
            if token.is_none() {
                break;
            }
        }
        names.sort();
        Ok(names)
    }

    /// Every class the store holds a directory for, as `(scope, class)`.
    ///
    /// The discovery source for background reconciliation. It covers the
    /// routine damage, a row missing because its object stopped writing
    /// and a row lingering because its object expired, both of which
    /// leave the class's prefix in place.
    ///
    /// What it cannot cover is a class whose prefix is gone entirely,
    /// since the prefix is the thing being asked. That is disaster
    /// recovery rather than reconciliation and belongs to an operator
    /// verb naming the class, which is also how the design frames it.
    /// The placement store is not the answer either: an unscoped
    /// `ListInstances` deliberately matches no rows, because falling
    /// back to every project's objects is the wrong default for a
    /// multi-tenant listing.
    ///
    /// # Errors
    /// Returns the store's message.
    pub async fn directory_classes(&self) -> Result<Vec<(String, String)>, String> {
        let mut classes = Vec::new();
        for scope in self.children("directory/").await? {
            let prefix = format!("directory/{scope}/");
            for class in self.children(&prefix).await? {
                classes.push((scope.clone(), class));
            }
        }
        Ok(classes)
    }

    /// One level of key segments below `prefix`, via the delimiter, so
    /// a class listing never pages the whole tree.
    async fn children(&self, prefix: &str) -> Result<Vec<String>, String> {
        let mut names = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let page = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(prefix)
                .delimiter("/")
                .set_continuation_token(token)
                .send()
                .await
                .map_err(|e| e.into_service_error().to_string())?;
            for common in page.common_prefixes() {
                if let Some(name) = common
                    .prefix()
                    .and_then(|full| full.strip_prefix(prefix))
                    .and_then(|rest| rest.strip_suffix('/'))
                {
                    names.push(name.to_owned());
                }
            }
            token = page.next_continuation_token().map(str::to_owned);
            if token.is_none() {
                break;
            }
        }
        Ok(names)
    }

    /// Bytes of one delta or base, both content-addressed and
    /// immutable, so the answer is cached under the key forever.
    ///
    /// Concurrent misses of one key collapse into a single fetch, and a
    /// failed fetch is not cached, so a flaky store costs retries
    /// rather than a poisoned entry.
    ///
    /// # Errors
    /// Returns the store's message.
    pub async fn directory_file(
        &self,
        scope_id: &str,
        class: &str,
        folder: &str,
        name: &str,
    ) -> Result<Arc<[u8]>, String> {
        let key = format!(
            "{}{folder}/{name}.sqlite",
            Self::directory_prefix(scope_id, class)
        );
        self.file_reads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.files
            .try_get_with(key.clone(), async {
                self.file_fetches
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let bytes = self.get(key.clone()).await?;
                // Only a miss reaches the store, so this line counts
                // the downloads a class actually costs.
                actias_common::tracing::debug!(
                    key = %key,
                    bytes = bytes.len(),
                    "a directory file was fetched from the store"
                );
                Ok::<_, String>(Arc::from(bytes))
            })
            .await
            .map_err(|error: Arc<String>| error.to_string())
    }

    /// Uploads a merged base under its content hash.
    ///
    /// # Errors
    /// Returns the store's message.
    pub async fn put_directory_base(
        &self,
        scope_id: &str,
        class: &str,
        name: &str,
        bytes: Vec<u8>,
    ) -> Result<(), String> {
        self.put(
            format!(
                "{}bases/{name}.sqlite",
                Self::directory_prefix(scope_id, class)
            ),
            bytes,
        )
        .await
    }

    /// Writes the class's manifest, refusing to move it backwards.
    ///
    /// The generation is the fence: a compactor whose lease lapsed
    /// while it merged finds a newer generation already published and
    /// loses, exactly as a zombie shipper loses to a newer epoch. Its
    /// base is orphaned rather than referenced, and generation GC
    /// collects it, which is why bases are content-addressed: two
    /// racing compactors write disjoint keys and cannot corrupt each
    /// other's bytes.
    ///
    /// # Errors
    /// Returns a refusal when a newer generation already exists, or the
    /// store's message.
    pub async fn put_directory_manifest(
        &self,
        scope_id: &str,
        class: &str,
        manifest: &DirectoryManifest,
    ) -> Result<(), String> {
        if let Some(current) = self.directory_manifest(scope_id, class).await?
            && current.generation >= manifest.generation
        {
            return Err(format!(
                "fenced: generation {} lost to {}; another compactor published first",
                manifest.generation, current.generation
            ));
        }
        let bytes = serde_json::to_vec(manifest).map_err(|e| e.to_string())?;
        self.put(
            format!("{}manifest.json", Self::directory_prefix(scope_id, class)),
            bytes,
        )
        .await
    }

    /// Removes folded deltas and superseded bases. Keeps the base the
    /// manifest names and the one before it as the rollback margin, and
    /// keeps every delta the manifest has not folded, including ones
    /// written while the merge ran.
    ///
    /// # Errors
    /// Returns the store's message.
    pub async fn collect_directory_garbage(
        &self,
        scope_id: &str,
        class: &str,
        manifest: &DirectoryManifest,
        keep_base: Option<&str>,
    ) -> Result<(), String> {
        let prefix = Self::directory_prefix(scope_id, class);
        let folded: std::collections::HashSet<&str> =
            manifest.folded.iter().map(String::as_str).collect();
        let mut doomed = Vec::new();

        for name in self.directory_deltas(scope_id, class).await? {
            if folded.contains(name.as_str()) {
                doomed.push(format!("{prefix}deltas/{name}.sqlite"));
            }
        }

        let bases_prefix = format!("{prefix}bases/");
        let mut token: Option<String> = None;
        loop {
            let page = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(&bases_prefix)
                .set_continuation_token(token)
                .send()
                .await
                .map_err(|e| e.into_service_error().to_string())?;
            for key in page.contents().iter().filter_map(|object| object.key()) {
                let Some(name) = key
                    .strip_prefix(&bases_prefix)
                    .and_then(|rest| rest.strip_suffix(".sqlite"))
                else {
                    continue;
                };
                let current = manifest.base.as_deref() == Some(name);
                let margin = keep_base == Some(name);
                if !current && !margin {
                    doomed.push(key.to_owned());
                }
            }
            token = page.next_continuation_token().map(str::to_owned);
            if token.is_none() {
                break;
            }
        }

        for key in doomed {
            self.client
                .delete_object()
                .bucket(&self.bucket)
                .key(&key)
                .send()
                .await
                .map_err(|e| e.into_service_error().to_string())?;
        }
        Ok(())
    }

    async fn put_manifest(&self, object_id: &str, manifest: &Manifest) -> Result<(), String> {
        let bytes = serde_json::to_vec(manifest).map_err(|e| e.to_string())?;
        self.put(Self::manifest_key(object_id), bytes).await
    }

    async fn get(&self, key: String) -> Result<Vec<u8>, String> {
        let object = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| e.into_service_error().to_string())?;
        Ok(object
            .body
            .collect()
            .await
            .map_err(|e| e.to_string())?
            .into_bytes()
            .to_vec())
    }
}

/// What `gen.json` records for one generation: enough for garbage
/// collection to know what the rollback margin references without the
/// manifest.
#[derive(Serialize, Deserialize)]
struct GenerationRecord {
    base_len: u64,
    chunks: Vec<String>,
}

/// What a checkpoint left: the new chunk list, which entries changed,
/// the file's length, and the directory row read on the way.
struct Folded {
    chunks: Vec<String>,
    dirty: Vec<u32>,
    base_len: u64,
    directory: DirectoryRow,
}

/// Folds the WAL into the main file and hashes what changed. The dirty
/// set comes from the frames themselves: every frame names its page,
/// so the chunks the checkpoint is about to rewrite are known before it
/// runs. A PASSIVE checkpoint does the bulk of the fold without the
/// write lock; the TRUNCATE after it folds what landed meanwhile and
/// empties the WAL. With no previous list, or a WAL from another
/// incarnation than the previous generation's, every chunk is hashed.
fn fold_and_hash(
    file: &Path,
    prev_chunks: &[String],
    prev_salts: Option<(u32, u32)>,
) -> Result<Folded, String> {
    let wal = match std::fs::read(wal_path(file)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error.to_string()),
    };
    let mut everything = prev_chunks.is_empty();
    let mut dirty: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
    if !everything {
        match actias_worker_core::wal::frames(&wal) {
            Ok((prefix, frames)) => {
                if prev_salts.is_some_and(|salts| salts != prefix.salts) {
                    everything = true;
                } else {
                    let page_size = prefix.page_size as u64;
                    for frame in frames {
                        let at = (frame.page as u64 - 1) * page_size;
                        dirty.insert((at / CHUNK_BYTES) as u32);
                    }
                }
            }
            Err(actias_worker_core::wal::WalError::NotAWal) if wal.len() < 32 => {}
            Err(_) => everything = true,
        }
    }

    let mut storage = actias_worker_core::storage::SqliteStorage::open(file)?;
    storage.checkpoint_passive()?;
    storage.checkpoint()?;
    let directory = read_directory_row(&mut storage);
    drop(storage);

    let base_len = std::fs::metadata(file).map_err(|e| e.to_string())?.len();
    let count = chunk_count(base_len);
    let source = std::fs::File::open(file).map_err(|e| e.to_string())?;
    let mut chunks = Vec::with_capacity(count);
    let mut changed = Vec::new();
    let mut buffer = vec![0u8; CHUNK_BYTES as usize];
    for index in 0..count {
        let known = prev_chunks
            .get(index)
            .filter(|_| !everything && !dirty.contains(&(index as u32)));
        match known {
            Some(hash) => chunks.push(hash.clone()),
            None => {
                use std::os::unix::fs::FileExt;
                let at = index as u64 * CHUNK_BYTES;
                let len = (base_len - at).min(CHUNK_BYTES) as usize;
                source
                    .read_exact_at(&mut buffer[..len], at)
                    .map_err(|e| e.to_string())?;
                chunks.push(blake3::hash(&buffer[..len]).to_hex().to_string());
                changed.push(index as u32);
            }
        }
    }
    Ok(Folded {
        chunks,
        dirty: changed,
        base_len,
        directory,
    })
}

/// One chunk of the main file, read off the caller's thread.
pub(crate) async fn read_chunk(file: &Path, index: u32) -> Result<Vec<u8>, String> {
    let file = file.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        use std::os::unix::fs::FileExt;
        let source = std::fs::File::open(&file).map_err(|e| e.to_string())?;
        let len = source.metadata().map_err(|e| e.to_string())?.len();
        let at = index as u64 * CHUNK_BYTES;
        if at >= len {
            return Err(format!("chunk {index} is past the file"));
        }
        let mut bytes = vec![0u8; (len - at).min(CHUNK_BYTES) as usize];
        source
            .read_exact_at(&mut bytes, at)
            .map_err(|e| e.to_string())?;
        Ok(bytes)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// A generation directory's name back as (epoch, base).
fn parse_generation(dir: &str) -> Option<(u64, u64)> {
    let (epoch, base) = dir.strip_prefix('e')?.split_once("-b")?;
    Some((epoch.parse().ok()?, base.parse().ok()?))
}

/// The WAL beside an object file. Appended, not `with_extension`:
/// object file names come from user-chosen text and may contain dots.
fn wal_path(file: &Path) -> PathBuf {
    let mut path = file.as_os_str().to_owned();
    path.push("-wal");
    PathBuf::from(path)
}

fn shm_path(file: &Path) -> PathBuf {
    let mut path = file.as_os_str().to_owned();
    path.push("-shm");
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deletion_marker_outranks_the_epoch_it_fences() {
        let marker: Manifest = serde_json::from_str(
            r#"{ "version": 3, "epoch": 6, "shipped_at": 2, "deleted": true }"#,
        )
        .expect("marker parses");
        assert!(marker.deleted);
        assert!(marker.chunks.is_empty() && marker.base_len == 0);
        let live: Manifest = serde_json::from_str(
            r#"{ "version": 3, "epoch": 5, "shipped_at": 1, "base_len": 4096, "chunks": ["ab"] }"#,
        )
        .expect("a live manifest parses");
        assert!(!live.deleted);
        assert_eq!(live.chunks, vec!["ab".to_owned()]);
        // The fence a zombie's late ship must lose: a marker at the
        // bumped epoch beats the epoch the zombie still holds.
        assert!(marker.epoch > live.epoch);
    }

    #[test]
    fn a_manifest_without_a_directory_row_still_reads() {
        let v3: Manifest = serde_json::from_str(
            r#"{ "version": 3, "epoch": 6, "shipped_at": 2, "base": 1, "segments": 3 }"#,
        )
        .expect("a manifest without the field parses");
        assert!(v3.directory.is_none());
    }

    #[tokio::test]
    async fn the_dirty_set_is_the_frames_pages_and_nothing_else() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("o.db");
        // A base of several chunks: rows of a fixed size until the file
        // is past two chunks, checkpointed.
        {
            let mut storage =
                actias_worker_core::storage::SqliteStorage::open(&file).expect("opens");
            storage
                .exec("CREATE TABLE t (k INTEGER PRIMARY KEY, v BLOB)", &[])
                .expect("schema");
            let blob = "x".repeat(4000);
            for k in 0..600 {
                storage
                    .exec(
                        "INSERT INTO t VALUES (?, ?)",
                        &[serde_json::json!(k), serde_json::json!(blob)],
                    )
                    .expect("row");
            }
        }
        // The object's own connection stays open across the flights, as
        // the task's does for a residency.
        let _resident = actias_worker_core::storage::SqliteStorage::open(&file).expect("opens");
        let first = fold_and_hash(&file, &[], None).expect("folds");
        assert!(first.chunks.len() >= 3, "the base spans chunks");
        assert_eq!(first.dirty.len(), first.chunks.len(), "everything hashed");
        assert_eq!(
            first.base_len,
            std::fs::metadata(&file).expect("meta").len()
        );

        // One row in the first table page rewritten: its chunk is dirty,
        // the rest keep their hashes.
        {
            let mut storage =
                actias_worker_core::storage::SqliteStorage::open(&file).expect("opens");
            storage
                .exec(
                    "UPDATE t SET v = ? WHERE k = 1",
                    &[serde_json::json!("y".repeat(4000))],
                )
                .expect("update");
        }
        let salts = {
            let wal = std::fs::read(wal_path(&file)).expect("wal");
            actias_worker_core::wal::committed_prefix(&wal)
                .expect("prefix")
                .salts
        };
        let second = fold_and_hash(&file, &first.chunks, Some(salts)).expect("folds");
        assert_eq!(second.chunks.len(), first.chunks.len());
        assert!(!second.dirty.is_empty() && second.dirty.len() < first.chunks.len());
        for (index, hash) in second.chunks.iter().enumerate() {
            if second.dirty.contains(&(index as u32)) {
                assert_ne!(hash, &first.chunks[index], "a dirty chunk rehashed");
            } else {
                assert_eq!(hash, &first.chunks[index], "a clean chunk kept its hash");
            }
        }
        // The dirty hashes are the file's actual content.
        for index in &second.dirty {
            let bytes = read_chunk(&file, *index).await.expect("chunk");
            assert_eq!(
                blake3::hash(&bytes).to_hex().to_string(),
                second.chunks[*index as usize]
            );
        }

        // A WAL from another incarnation than the one the list was
        // built under hashes everything again.
        {
            let mut storage =
                actias_worker_core::storage::SqliteStorage::open(&file).expect("opens");
            storage
                .exec(
                    "UPDATE t SET v = ? WHERE k = 2",
                    &[serde_json::json!("z".repeat(4000))],
                )
                .expect("update");
        }
        let third = fold_and_hash(&file, &second.chunks, Some((1, 2))).expect("folds");
        assert_eq!(third.dirty.len(), third.chunks.len());
        // What it hashed is what a fresh hash of the folded file says.
        let fresh = fold_and_hash(&file, &[], None).expect("folds");
        assert_eq!(list_hash(&third.chunks), list_hash(&fresh.chunks));
        assert_ne!(
            list_hash(&third.chunks),
            list_hash(&second.chunks),
            "the update changed the file"
        );
    }

    /// The row survives the manifest round trip with its fields, its
    /// rev and its failure marker: this is what makes a repair a
    /// metadata copy rather than a restore.
    #[test]
    fn a_carried_row_round_trips_through_the_manifest() {
        use actias_worker_core::directory::row::{Pair, RowSnapshot};

        let manifest = Manifest {
            version: 3,
            epoch: 7,
            shipped_at: 3,
            base: 0,
            segments: 0,
            deleted: false,
            replicas: vec!["n1".to_owned()],
            wal_len: 0,
            base_len: 0,
            chunks: Vec::new(),
            directory: Some(RowSnapshot {
                rev: 42,
                dver: 0,
                fields: vec![Pair {
                    field: "status".to_owned(),
                    kind: "string".to_owned(),
                    value: "open".to_owned(),
                }],
                failed: Some((43, 0)),
            }),
        };

        let text = serde_json::to_string(&manifest).expect("serializes");
        let read: Manifest = serde_json::from_str(&text).expect("parses back");
        let row = read.directory.expect("the row survives");
        assert_eq!(row.rev, 42);
        assert_eq!(row.failed, Some((43, 0)));
        assert_eq!(row.fields.len(), 1);
        assert_eq!(row.fields[0].field, "status");
        assert_eq!(row.fields[0].value, "open");

        // A row-less manifest does not carry the key at all, so the
        // common case costs nothing on the wire.
        let bare = Manifest {
            directory: None,
            ..manifest
        };
        assert!(
            !serde_json::to_string(&bare)
                .expect("serializes")
                .contains("directory")
        );
    }
}
