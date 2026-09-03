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
    #[serde(default = "two")]
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
}

fn two() -> u32 {
    2
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
    /// A WAL this large rotates the generation at the next flight.
    pub rotate_bytes: u64,
    /// So does this many segments, whatever their size.
    pub max_segments: u32,
}

/// One fan-out: the generation, the committed WAL prefix as a whole (so
/// a replica behind can be resent from wherever it is), the offset the
/// owner believes the replicas hold, and the base when a generation
/// starts.
pub struct FanoutRequest {
    pub epoch: u64,
    pub base: u64,
    pub offset: u64,
    pub wal: Bytes,
    pub base_bytes: Option<Bytes>,
    pub covered: u64,
    /// Acks that answer the flight; the fan-out stops waiting on the
    /// rest soon after it has them. 0 waits for everyone.
    pub quorum: usize,
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
    pub node_ids: Vec<String>,
    pub quorum: usize,
    pub release: std::sync::Mutex<Option<crate::objects::shipper::Release>>,
    pub gauges: Arc<crate::objects::replica::ReplicaGauges>,
    /// Set when a fan-out came back below the quorum: the replica fence
    /// could not have spoken, so the store's fence is re-read before the
    /// next put. Shared across a residency's flights.
    pub short: Arc<std::sync::atomic::AtomicBool>,
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
struct Generation {
    base: u64,
    segments: u32,
    /// The WAL incarnation the offsets count against; learned from the
    /// first frames after the generation's checkpoint.
    salts: Option<(u32, u32)>,
    /// Shipped through this byte of the WAL file.
    offset: usize,
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
}

impl ObjectStore {
    pub fn new(client: aws_sdk_s3::Client, bucket: String, cache_bytes: u64) -> Self {
        Self {
            client,
            bucket,
            files: moka::future::Cache::builder()
                .max_capacity(cache_bytes)
                .weigher(|_, bytes: &Arc<[u8]>| bytes.len().clamp(1, u32::MAX as usize) as u32)
                .build(),
            file_reads: std::sync::atomic::AtomicU64::new(0),
            file_fetches: std::sync::atomic::AtomicU64::new(0),
        }
    }

    fn manifest_key(object_id: &str) -> String {
        format!("objects/{object_id}/manifest.json")
    }

    fn base_key(object_id: &str, epoch: u64, base: u64) -> String {
        format!("objects/{object_id}/e{epoch}-b{base}/base.db")
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
    /// as the base. Every flight after ships the committed WAL frames
    /// since the last one as one segment, and the generation rotates
    /// when the WAL or the segment count grows past the thresholds. A
    /// segment that cannot be cut (the WAL restarted behind the shipper)
    /// starts the next generation instead, which folds every frame in.
    /// A flight that lays a new generation sweeps old ones, keeping the
    /// newest previous as the rollback margin.
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

        let next = match state.generation.as_ref() {
            None => {
                let base = state.must_rotate.unwrap_or(0);
                state.must_rotate = Some(base);
                self.start_generation(object_id, epoch, file, base, replication, may_release)
                    .await
            }
            Some(generation)
                if state.must_rotate.is_some()
                    || generation.segments >= thresholds.max_segments
                    || wal_len >= thresholds.rotate_bytes =>
            {
                // Rotation: the checkpoint folds every frame, shipped or
                // not, into the next base; nothing can be lost to the
                // boundary. A failed rotation keeps the generation as it
                // was, and `must_rotate` remembers that the file was
                // checkpointed, so the retry lays the generation rather
                // than reading an empty WAL as "nothing to ship".
                let base = state.must_rotate.unwrap_or(generation.base + 1);
                state.must_rotate = Some(base);
                self.start_generation(object_id, epoch, file, base, replication, may_release)
                    .await
            }
            Some(generation) => {
                let base = generation.base;
                let resumed = Generation {
                    base: generation.base,
                    segments: generation.segments,
                    salts: generation.salts,
                    offset: generation.offset,
                };
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
                        state.must_rotate = Some(base + 1);
                        self.start_generation(
                            object_id,
                            epoch,
                            file,
                            base + 1,
                            replication,
                            may_release,
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
            && let Some(base) = base_after
            && let Err(error) = self.collect_garbage(object_id, (epoch, base)).await
        {
            actias_common::tracing::warn!(object_id, %error, "generation sweep failed");
        }
        Ok(())
    }

    /// Deletes everything under the object except the current
    /// generation, the newest one before it (the rollback margin while
    /// the current one is young), and the manifest.
    async fn collect_garbage(&self, object_id: &str, current: (u64, u64)) -> Result<(), String> {
        let prefix = format!("objects/{object_id}/");
        let mut generations: std::collections::BTreeMap<(u64, u64), Vec<String>> =
            std::collections::BTreeMap::new();
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
                if let Some(generation) = rest
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
        if let Some(newest) = generations.keys().next_back().copied() {
            generations.remove(&newest);
        }
        for key in generations.into_values().flatten() {
            self.client
                .delete_object()
                .bucket(&self.bucket)
                .key(key)
                .send()
                .await
                .map_err(|e| e.into_service_error().to_string())?;
        }
        Ok(())
    }

    /// Starts a segment generation: fold the WAL at a TRUNCATE
    /// checkpoint, ship the main file as the base, and count frames
    /// from zero. The raw copy of the main file is safe exactly here:
    /// only the shipper checkpoints, and its flights never overlap, so
    /// between checkpoints the main file does not change.
    async fn start_generation(
        &self,
        object_id: &str,
        epoch: u64,
        file: &Path,
        base: u64,
        replication: Option<&Replication>,
        may_release: bool,
    ) -> Result<Generation, String> {
        let source = file.to_path_buf();
        let (bytes, directory) =
            tokio::task::spawn_blocking(move || -> Result<(Vec<u8>, DirectoryRow), String> {
                let mut storage = actias_worker_core::storage::SqliteStorage::open(&source)?;
                storage.checkpoint()?;
                let directory = read_directory_row(&mut storage);
                let bytes = std::fs::read(&source).map_err(|e| e.to_string())?;
                Ok((bytes, directory))
            })
            .await
            .map_err(|e| e.to_string())??;

        // The replicas get the base before the store does; a generation
        // start is the one flight where a replica needs more than an
        // append.
        // A generation start releases at the manifest, never on the
        // quorum: a takeover asks replicas for the generation the
        // manifest names, and a quorum holding a generation no manifest
        // names yet is a quorum nobody can find.
        let _ = may_release;
        let bytes = Bytes::from(bytes);
        if let Some(replication) = replication {
            replication
                .send(
                    FanoutRequest {
                        epoch,
                        base,
                        offset: 0,
                        wal: Bytes::new(),
                        base_bytes: Some(bytes.clone()),
                        covered: 0,
                        quorum: replication.quorum,
                    },
                    false,
                )
                .await?;
        }
        self.put(Self::base_key(object_id, epoch, base), bytes.to_vec())
            .await?;
        self.put_manifest(
            object_id,
            &Manifest {
                version: 2,
                epoch,
                shipped_at: actias_worker_core::extensions::objects::unix_now_ms(),
                base,
                segments: 0,
                deleted: false,
                directory,
                replicas: replication.map(|r| r.node_ids.clone()).unwrap_or_default(),
                wal_len: 0,
            },
        )
        .await?;
        Ok(Generation {
            base,
            segments: 0,
            salts: None,
            offset: 0,
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
                        offset: offset as u64,
                        wal: Bytes::from(prefix_bytes),
                        base_bytes: None,
                        covered: offset as u64,
                        quorum: replication.quorum,
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
                version: 2,
                epoch,
                shipped_at: actias_worker_core::extensions::objects::unix_now_ms(),
                base,
                segments: segments + 1,
                deleted: false,
                directory,
                replicas: replication.map(|r| r.node_ids.clone()).unwrap_or_default(),
                wal_len: prefix.len as u64,
            },
        )
        .await?;
        actias_worker_core::drill::fault("after-manifest");
        Ok(Generation {
            base,
            segments: segments + 1,
            salts: Some(prefix.salts),
            offset: prefix.len,
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
                version: 2,
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

        let base = self
            .get(Self::base_key(object_id, manifest.epoch, manifest.base))
            .await?;
        tokio::fs::write(file, base)
            .await
            .map_err(|e| e.to_string())?;

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
                serde_json::from_slice(&bytes)
                    .map(Some)
                    .map_err(|e| e.to_string())
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
    fn manifests_from_before_the_marker_read_as_live() {
        let old: Manifest = serde_json::from_str(r#"{ "epoch": 5, "shipped_at": 1 }"#)
            .expect("a bare manifest parses");
        assert_eq!(old.version, 2);
        assert!(!old.deleted, "absence of the field means live data");
        assert!(
            old.replicas.is_empty(),
            "absence of the field means no replicas"
        );

        let marker: Manifest = serde_json::from_str(
            r#"{ "version": 2, "epoch": 6, "shipped_at": 2, "deleted": true }"#,
        )
        .expect("marker parses");
        assert!(marker.deleted);

        // The fence a zombie's late ship must lose: a marker at the
        // bumped epoch beats the epoch the zombie still holds.
        assert!(marker.epoch > old.epoch);
    }

    /// The directory row is additive on the manifest, so it needs no
    /// version ladder: every manifest written before the field reads
    /// as carrying no row, which is exactly true of anything shipped
    /// before its class had a directory.
    #[test]
    fn a_manifest_without_a_directory_row_still_reads() {
        let old: Manifest = serde_json::from_str(r#"{ "epoch": 5, "shipped_at": 1 }"#)
            .expect("a v1 manifest parses");
        assert!(old.directory.is_none());

        let v2: Manifest = serde_json::from_str(
            r#"{ "version": 2, "epoch": 6, "shipped_at": 2, "base": 1, "segments": 3 }"#,
        )
        .expect("a v2 manifest without the field parses");
        assert!(v2.directory.is_none());
    }

    /// The row survives the manifest round trip with its fields, its
    /// rev and its failure marker: this is what makes a repair a
    /// metadata copy rather than a restore.
    #[test]
    fn a_carried_row_round_trips_through_the_manifest() {
        use actias_worker_core::directory::row::{Pair, RowSnapshot};

        let manifest = Manifest {
            version: 2,
            epoch: 7,
            shipped_at: 3,
            base: 0,
            segments: 0,
            deleted: false,
            replicas: vec!["n1".to_owned()],
            wal_len: 0,
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
