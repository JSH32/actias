//! The replica side of tail replication: what this node holds for
//! objects whose owners fan their WAL out to it. A replica never
//! interprets bytes. It keeps, per object, one generation as an
//! assembled `base.db` beside a growing `wal`, the chunk list the base
//! was laid from (`gen.json`), and a fence, the highest epoch it has
//! seen. It does four things: lay a generation from a stream of chunks
//! (patching its base in place when the stream is a delta over the
//! list it holds), append at an offset, answer what it holds, and hand
//! the copy over.
//!
//! Every operation on one object runs under that object's own lock,
//! from the first read of its state to the last write of it, so an
//! append, a read copy, a takeover's fence and the eviction sweep can
//! never interleave on the same object: a WAL length only grows, a
//! fence only rises, and a directory is deleted only by an operation
//! that holds the lock while it looks.
//!
//! Durability of an append is an `fdatasync`, batched: appends that
//! arrive within one round of the sync loop share the disk's round
//! trip, across every object this node replicates. `SyncMode::Os` acks
//! from the page cache instead, for a deployment that wants it and says
//! so.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use tokio::sync::{Mutex, OwnedMutexGuard, mpsc, oneshot};

/// How an append becomes durable before it is acked.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SyncMode {
    /// `fdatasync` the WAL, batched with whatever else arrived.
    Fsync,
    /// Trust the OS; the ack means "in the page cache".
    Os,
}

impl SyncMode {
    pub fn parse(text: &str) -> Self {
        if text.eq_ignore_ascii_case("os") {
            Self::Os
        } else {
            Self::Fsync
        }
    }
}

/// Node-wide replica counters for `/_metrics`.
#[derive(Default)]
pub struct ReplicaGauges {
    pub appends: AtomicU64,
    pub append_refusals: AtomicU64,
    pub append_ms_total: AtomicU64,
    /// WAL and base bytes held for other nodes' objects.
    pub bytes_held: AtomicI64,
    pub objects_held: AtomicI64,
    /// Copies handed over to a takeover.
    pub fetches: AtomicU64,
    /// Generations laid from an owner's stream, the chunks they carried
    /// and their bytes; a delta lay carries only the dirty chunks.
    pub lays: AtomicU64,
    pub lay_chunks: AtomicU64,
    pub lay_bytes: AtomicU64,
    /// Reads served from this node's copy after the owner's watermark
    /// confirmed it, reads that waited for an append to land first, and
    /// reads forwarded to the owner because the copy could not be
    /// confirmed in time.
    pub reads_confirmed: AtomicU64,
    pub reads_waited: AtomicU64,
    pub reads_forwarded: AtomicU64,
    /// The owner side: appends this node fanned out, their acks' cost,
    /// and the ones that failed or were refused.
    pub fanout_appends: AtomicU64,
    pub fanout_ack_ms_total: AtomicU64,
    pub fanout_failures: AtomicU64,
    /// Flights whose tickets were released on a replica quorum, and
    /// flights released at the store's manifest as before.
    pub quorum_releases: AtomicU64,
    pub store_releases: AtomicU64,
    /// Residencies laid from a replica, and the ones that had to fall
    /// back to the store because too few replicas answered.
    pub takeovers: AtomicU64,
    pub takeover_incidents: AtomicU64,
}

/// Why an append was refused, as the wire carries it.
pub mod refusal {
    /// The epoch is below the replica's fence, or the generation is
    /// older than the one held.
    pub const FENCED: &str = "fenced";
    /// The offset is past what the replica holds; resend from `length`.
    pub const GAP: &str = "gap";
    /// The replica lacks the generation, or is not on the list a delta
    /// applies to; lay it, every chunk.
    pub const NO_BASE: &str = "base";
}

/// One generation this node holds for an object.
#[derive(Clone, Debug)]
struct Held {
    epoch: u64,
    base: u64,
    wal_len: u64,
    base_len: u64,
    /// The chunk list the base was laid from; a delta must name it.
    chunks: Vec<String>,
    /// How far the owner said the store's manifest covers this WAL; a
    /// copy the store covers entirely may leave once idle.
    covered: u64,
    /// The WAL length the read copy reflects; [`None`] before the base
    /// was copied for this generation.
    read_len: Option<u64>,
    last_append: std::time::Instant,
}

#[derive(Clone, Debug, Default)]
struct Meta {
    /// Highest epoch seen for the object; appends below it refuse.
    fence: u64,
    held: Option<Held>,
}

/// A generation lay's header, as the stream carries it.
pub struct LayHeader {
    pub epoch: u64,
    pub base: u64,
    /// The list hash the delta applies to; empty for a full lay.
    pub from_list: String,
    pub base_len: u64,
    pub chunks: Vec<String>,
}

/// One chunk of a lay: its index and bytes.
pub type LayPart = Result<(u32, actias_worker_core::proto::Bytes), String>;

/// A held copy's files, for a takeover to stream.
pub struct Copy {
    pub base: PathBuf,
    pub base_len: u64,
    pub wal: PathBuf,
    pub wal_len: u64,
}

/// The outcome of one append or lay.
#[derive(Debug, PartialEq, Eq)]
pub struct Appended {
    pub length: u64,
    pub applied: bool,
    /// One of [`refusal`]'s codes when not applied.
    pub refusal: String,
}

/// What a replica answers about one generation.
#[derive(Debug, PartialEq, Eq)]
pub struct Info {
    pub held: bool,
    pub length: u64,
    pub fence: u64,
}

struct SyncRequest {
    path: PathBuf,
    done: oneshot::Sender<Result<(), String>>,
}

/// What the store's manifest says about an object's generation, asked
/// by the eviction sweep for a copy whose owner stopped appending
/// before it could say so.
pub enum Cover {
    /// The store's segments cover the generation's WAL through this
    /// length.
    Through(u64),
    /// The identity was forgotten: the store holds a deletion marker,
    /// and a replica has nothing to keep, fence included.
    Forgotten,
    /// The store holds no manifest for that generation.
    Unknown,
}

/// Asks the store about one generation; the worker answers from the
/// manifest, tests from anything.
pub type Coverage = Arc<
    dyn Fn(String, u64, u64) -> std::pin::Pin<Box<dyn std::future::Future<Output = Cover> + Send>>
        + Send
        + Sync,
>;

pub struct ReplicaStore {
    dir: PathBuf,
    mode: SyncMode,
    /// Idle time after which a fully covered copy leaves the disk.
    idle: std::time::Duration,
    /// Per-object state behind its own lock. An entry exists only for
    /// objects this node holds or has fenced; a miss for anything else
    /// costs a directory look and caches nothing, so the map is bounded
    /// by what is here.
    slots: Mutex<HashMap<String, Arc<Mutex<Meta>>>>,
    syncer: mpsc::UnboundedSender<SyncRequest>,
    pub gauges: Arc<ReplicaGauges>,
}

/// How often idle copies are considered for eviction.
const EVICTION_SWEEP: std::time::Duration = std::time::Duration::from_secs(60);

impl ReplicaStore {
    /// Opens the store under `dir` and starts its sync loop. The
    /// eviction sweep starts with [`Self::start_sweep`].
    pub fn new(dir: PathBuf, mode: SyncMode, idle: std::time::Duration) -> Arc<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        let store = Arc::new(Self {
            dir,
            mode,
            idle,
            slots: Mutex::new(HashMap::new()),
            syncer: tx,
            gauges: Arc::default(),
        });
        tokio::spawn(sync_loop(rx));
        store
    }

    /// Runs the eviction sweep forever, with `coverage` to ask the store
    /// how far it holds a copy whose owner went quiet.
    pub fn start_sweep(self: &Arc<Self>, coverage: Coverage) {
        let store = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(EVICTION_SWEEP).await;
                store.evict_idle(&coverage).await;
            }
        });
    }

    fn object_dir(&self, object_id: &str) -> PathBuf {
        self.dir.join(object_id)
    }

    fn generation_dir(&self, object_id: &str, epoch: u64, base: u64) -> PathBuf {
        self.object_dir(object_id).join(format!("e{epoch}-b{base}"))
    }

    /// The object's state under its lock, for the whole operation. On a
    /// miss the state is read from disk; nothing is cached for an object
    /// that has neither a copy nor a fence unless `create` asks for it.
    async fn lock(&self, object_id: &str, create: bool) -> Option<OwnedMutexGuard<Meta>> {
        let slot = {
            let mut slots = self.slots.lock().await;
            match slots.get(object_id) {
                Some(slot) => slot.clone(),
                None => {
                    let dir = self.object_dir(object_id);
                    let loaded = tokio::task::spawn_blocking(move || load_meta(&dir))
                        .await
                        .unwrap_or_default();
                    if loaded.held.is_none() && loaded.fence == 0 && !create {
                        return None;
                    }
                    if let Some(held) = &loaded.held {
                        self.gauges.objects_held.fetch_add(1, Ordering::Relaxed);
                        self.gauges
                            .bytes_held
                            .fetch_add((held.wal_len + held.base_len) as i64, Ordering::Relaxed);
                    }
                    let slot = Arc::new(Mutex::new(loaded));
                    slots.insert(object_id.to_owned(), slot.clone());
                    slot
                }
            }
        };
        Some(slot.lock_owned().await)
    }

    /// Lays a generation from an owner's stream: the header names the
    /// new chunk list, `parts` carry chunks by index. A header whose
    /// `from_list` names the held list is a delta: the parts are written
    /// over the held base in place. An empty `from_list` is a full lay.
    /// Anything else is refused `NO_BASE`, and the owner sends everything.
    ///
    /// Crash safety: a `rotating` marker stands in the object directory
    /// while the base is being patched, and a copy found with the
    /// marker at load is forgotten. Forgetting is always safe; the store
    /// and the quorum's other members hold the bytes.
    ///
    /// # Errors
    /// Returns the io failure or a stream that ended before `done`.
    pub async fn lay<S>(
        &self,
        object_id: &str,
        header: LayHeader,
        mut parts: S,
    ) -> Result<Appended, String>
    where
        S: futures::Stream<Item = LayPart> + Unpin,
    {
        use futures::StreamExt;
        let started = std::time::Instant::now();
        let mut meta = self
            .lock(object_id, true)
            .await
            .ok_or_else(|| "the replica slot could not be made".to_owned())?;
        let refused = |meta: &Meta, code: &str| {
            self.gauges.append_refusals.fetch_add(1, Ordering::Relaxed);
            Ok(Appended {
                length: meta.held.as_ref().map_or(0, |h| h.wal_len),
                applied: false,
                refusal: code.to_owned(),
            })
        };
        let LayHeader {
            epoch,
            base,
            from_list,
            base_len,
            chunks,
        } = header;
        if epoch < meta.fence {
            return refused(&meta, refusal::FENCED);
        }
        if let Some(held) = &meta.held
            && (epoch, base) < (held.epoch, held.base)
        {
            return refused(&meta, refusal::FENCED);
        }
        if chunks.len() != crate::objects::store::chunk_count(base_len) {
            return Err(format!(
                "a lay of {} bytes lists {} chunks",
                base_len,
                chunks.len()
            ));
        }
        let delta = !from_list.is_empty();
        if delta
            && !meta
                .held
                .as_ref()
                .is_some_and(|held| crate::objects::store::list_hash(&held.chunks) == from_list)
        {
            return refused(&meta, refusal::NO_BASE);
        }

        // The directory: renamed from the held generation for a delta,
        // fresh otherwise, under the marker either way.
        let object_dir = self.object_dir(object_id);
        let dir = self.generation_dir(object_id, epoch, base);
        let previous = meta.held.clone();
        let fence = meta.fence.max(epoch);
        {
            let object_dir = object_dir.clone();
            let dir = dir.clone();
            let previous_dir = previous
                .as_ref()
                .map(|prev| self.generation_dir(object_id, prev.epoch, prev.base));
            tokio::task::spawn_blocking(move || -> Result<(), String> {
                std::fs::create_dir_all(&object_dir).map_err(|e| e.to_string())?;
                std::fs::write(object_dir.join("rotating"), b"").map_err(|e| e.to_string())?;
                match previous_dir {
                    Some(prev) if delta && prev != dir => {
                        std::fs::rename(&prev, &dir).map_err(|e| e.to_string())?;
                    }
                    Some(prev) if !delta => {
                        if prev != dir {
                            let _ = std::fs::remove_dir_all(&prev);
                        }
                        let _ = std::fs::remove_dir_all(&dir);
                        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                    }
                    _ => {
                        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                    }
                }
                std::fs::write(object_dir.join("fence"), fence.to_string())
                    .map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| e.to_string())??;
        }
        // Bookkeeping follows the marker: from here the copy is either
        // completed below or forgotten at the next load.
        if let Some(prev) = &previous {
            self.gauges
                .bytes_held
                .fetch_sub((prev.wal_len + prev.base_len) as i64, Ordering::Relaxed);
        } else {
            self.gauges.objects_held.fetch_add(1, Ordering::Relaxed);
        }
        meta.fence = fence;
        meta.held = None;

        let base_file = Arc::new(
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(dir.join("base.db"))
                .map_err(|e| e.to_string())?,
        );
        let read_copy = dir.join("read.db");
        let read_file = if delta && read_copy.exists() {
            Some(Arc::new(
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(&read_copy)
                    .map_err(|e| e.to_string())?,
            ))
        } else {
            let _ = std::fs::remove_file(&read_copy);
            None
        };
        let mut laid = 0u64;
        let mut laid_bytes = 0u64;
        let outcome: Result<(), String> = async {
            while let Some(part) = parts.next().await {
                let (index, bytes) = part?;
                if index as usize >= chunks.len() {
                    return Err(format!("chunk {index} is past the list"));
                }
                let len = bytes.len() as u64;
                let at = index as u64 * crate::objects::store::CHUNK_BYTES;
                let base_file = base_file.clone();
                let read_file = read_file.clone();
                tokio::task::spawn_blocking(move || -> Result<(), String> {
                    use std::os::unix::fs::FileExt;
                    base_file
                        .write_all_at(&bytes, at)
                        .map_err(|e| e.to_string())?;
                    if let Some(read_file) = read_file {
                        read_file
                            .write_all_at(&bytes, at)
                            .map_err(|e| e.to_string())?;
                    }
                    Ok(())
                })
                .await
                .map_err(|e| e.to_string())??;
                laid += 1;
                laid_bytes += len;
            }
            Ok(())
        }
        .await;
        if let Err(error) = outcome {
            // The marker stays: the copy is forgotten at the next load,
            // and now, since the slot says nothing is held.
            let _ = tokio::fs::remove_dir_all(&dir).await;
            return Err(error);
        }

        let wal_len_reset = {
            let dir = dir.clone();
            let object_dir = object_dir.clone();
            let base_file = base_file.clone();
            let read_file = read_file.clone();
            let record = GenerationRecord {
                base_len,
                chunks: chunks.clone(),
            };
            let sync = self.mode == SyncMode::Fsync;
            tokio::task::spawn_blocking(move || -> Result<(), String> {
                base_file.set_len(base_len).map_err(|e| e.to_string())?;
                if let Some(read_file) = &read_file {
                    read_file.set_len(base_len).map_err(|e| e.to_string())?;
                    let _ = std::fs::remove_file(with_suffix(&dir.join("read.db"), "-wal"));
                    let _ = std::fs::remove_file(with_suffix(&dir.join("read.db"), "-shm"));
                }
                std::fs::write(dir.join("wal"), []).map_err(|e| e.to_string())?;
                std::fs::write(
                    dir.join("gen.json"),
                    serde_json::to_vec(&record).map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?;
                if sync {
                    base_file.sync_data().map_err(|e| e.to_string())?;
                    std::fs::File::open(dir.join("gen.json"))
                        .and_then(|f| f.sync_all())
                        .map_err(|e| e.to_string())?;
                }
                std::fs::remove_file(object_dir.join("rotating")).map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| e.to_string())?
        };
        wal_len_reset?;

        self.gauges
            .bytes_held
            .fetch_add(base_len as i64, Ordering::Relaxed);
        meta.held = Some(Held {
            epoch,
            base,
            wal_len: 0,
            base_len,
            chunks,
            covered: 0,
            read_len: read_file.is_some().then_some(0),
            last_append: std::time::Instant::now(),
        });
        self.gauges.lays.fetch_add(1, Ordering::Relaxed);
        self.gauges.lay_chunks.fetch_add(laid, Ordering::Relaxed);
        self.gauges
            .lay_bytes
            .fetch_add(laid_bytes, Ordering::Relaxed);
        self.gauges
            .append_ms_total
            .fetch_add(started.elapsed().as_millis() as u64, Ordering::Relaxed);
        Ok(Appended {
            length: 0,
            applied: true,
            refusal: String::new(),
        })
    }

    /// Appends `bytes` at `offset` of the generation's WAL.
    ///
    /// # Errors
    /// Returns the io failure; a refusal is not an error but an
    /// [`Appended`] with `applied` false and the reason.
    pub async fn append(
        &self,
        object_id: &str,
        epoch: u64,
        base: u64,
        offset: u64,
        bytes: &[u8],
        covered: u64,
    ) -> Result<Appended, String> {
        let started = std::time::Instant::now();
        let mut meta = self
            .lock(object_id, true)
            .await
            .ok_or_else(|| "the replica slot could not be made".to_owned())?;
        let refused = |meta: &Meta, code: &str| {
            self.gauges.append_refusals.fetch_add(1, Ordering::Relaxed);
            Ok(Appended {
                length: meta.held.as_ref().map_or(0, |h| h.wal_len),
                applied: false,
                refusal: code.to_owned(),
            })
        };
        if epoch < meta.fence {
            return refused(&meta, refusal::FENCED);
        }
        // A generation older than the one held is a stale owner's, even
        // under a fence that never rose: what is held says what is
        // current here.
        if let Some(held) = &meta.held
            && (epoch, base) < (held.epoch, held.base)
        {
            return refused(&meta, refusal::FENCED);
        }

        let dir = self.generation_dir(object_id, epoch, base);
        let Some(held) = meta.held.as_mut() else {
            return refused(&Meta::default(), refusal::NO_BASE);
        };
        if (held.epoch, held.base) != (epoch, base) {
            return refused(&Meta::default(), refusal::NO_BASE);
        }
        if offset > held.wal_len {
            let length = held.wal_len;
            self.gauges.append_refusals.fetch_add(1, Ordering::Relaxed);
            return Ok(Appended {
                length,
                applied: false,
                refusal: refusal::GAP.to_owned(),
            });
        }

        // Idempotent by offset: the part already held is skipped, and a
        // whole duplicate is an ack with the length as it stands.
        let already = (held.wal_len - offset) as usize;
        if already < bytes.len() {
            let fresh = bytes[already..].to_vec();
            let wal = dir.join("wal");
            let at = held.wal_len;
            tokio::task::spawn_blocking(move || -> Result<(), String> {
                use std::io::{Seek, SeekFrom, Write};
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .open(&wal)
                    .map_err(|e| e.to_string())?;
                file.seek(SeekFrom::Start(at)).map_err(|e| e.to_string())?;
                file.write_all(&fresh).map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| e.to_string())??;
            // The bytes are on disk whether or not the sync below lands:
            // the length moves now, and a failed sync fails the ack, not
            // the bookkeeping.
            let added = (bytes.len() - already) as u64;
            held.wal_len += added;
            self.gauges
                .bytes_held
                .fetch_add(added as i64, Ordering::Relaxed);
            if let Some(path) = self.sync_target(&dir) {
                self.request_sync(path).await?;
            }
        }
        held.covered = held.covered.max(covered);
        held.last_append = std::time::Instant::now();
        let length = held.wal_len;
        if epoch > meta.fence {
            // A raise persists before it is relied on.
            let object_dir = self.object_dir(object_id);
            tokio::task::spawn_blocking(move || {
                std::fs::write(object_dir.join("fence"), epoch.to_string())
                    .map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| e.to_string())??;
            meta.fence = epoch;
        }
        self.gauges.appends.fetch_add(1, Ordering::Relaxed);
        self.gauges
            .append_ms_total
            .fetch_add(started.elapsed().as_millis() as u64, Ordering::Relaxed);
        Ok(Appended {
            length,
            applied: true,
            refusal: String::new(),
        })
    }

    fn sync_target(&self, dir: &Path) -> Option<PathBuf> {
        (self.mode == SyncMode::Fsync).then(|| dir.join("wal"))
    }

    async fn request_sync(&self, path: PathBuf) -> Result<(), String> {
        let (done, wait) = oneshot::channel();
        self.syncer
            .send(SyncRequest { path, done })
            .map_err(|_| "the replica sync loop is gone".to_owned())?;
        wait.await
            .map_err(|_| "the replica sync loop dropped the request".to_owned())?
    }

    /// What this node holds for the generation, raising the fence to
    /// `fence_to` first (0 leaves it). A fence that cannot be persisted
    /// is an error: a takeover relies on it.
    ///
    /// # Errors
    /// Returns the failure to write the fence.
    pub async fn state(
        &self,
        object_id: &str,
        epoch: u64,
        base: u64,
        fence_to: u64,
    ) -> Result<Info, String> {
        let Some(mut meta) = self.lock(object_id, fence_to > 0).await else {
            return Ok(Info {
                held: false,
                length: 0,
                fence: 0,
            });
        };
        if fence_to > meta.fence {
            let object_dir = self.object_dir(object_id);
            tokio::task::spawn_blocking(move || -> Result<(), String> {
                std::fs::create_dir_all(&object_dir).map_err(|e| e.to_string())?;
                std::fs::write(object_dir.join("fence"), fence_to.to_string())
                    .map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| e.to_string())??;
            meta.fence = fence_to;
        }
        Ok(match &meta.held {
            Some(held) if (held.epoch, held.base) == (epoch, base) => Info {
                held: true,
                length: held.wal_len,
                fence: meta.fence,
            },
            _ => Info {
                held: false,
                length: 0,
                fence: meta.fence,
            },
        })
    }

    /// The generation and WAL length this node holds for the object.
    pub async fn held_generation(&self, object_id: &str) -> Option<(u64, u64, u64)> {
        let meta = self.lock(object_id, false).await?;
        meta.held.as_ref().map(|h| (h.epoch, h.base, h.wal_len))
    }

    /// Waits until the held copy of `(epoch, base)` reaches `length`;
    /// true when it did within `budget`. The append that brings it is
    /// already on its way from the owner, so the wait is the tail of one
    /// round trip.
    pub async fn wait_for(
        &self,
        object_id: &str,
        epoch: u64,
        base: u64,
        length: u64,
        budget: std::time::Duration,
    ) -> bool {
        let deadline = std::time::Instant::now() + budget;
        loop {
            match self.held_generation(object_id).await {
                Some((e, b, len)) if (e, b) == (epoch, base) && len >= length => return true,
                Some((e, b, _)) if (e, b) > (epoch, base) => return false,
                _ => {}
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
    }

    /// Where the copy is: the base file, the WAL file and how much of
    /// the WAL was acked (a write in progress past that is not part of
    /// the copy); [`None`] when the generation is not held. The caller
    /// streams the files; nothing reads them whole.
    pub async fn fetch(&self, object_id: &str, epoch: u64, base: u64) -> Option<Copy> {
        let meta = self.lock(object_id, false).await?;
        let held = meta.held.as_ref()?;
        if (held.epoch, held.base) != (epoch, base) {
            return None;
        }
        let dir = self.generation_dir(object_id, epoch, base);
        self.gauges.fetches.fetch_add(1, Ordering::Relaxed);
        Some(Copy {
            base: dir.join("base.db"),
            base_len: held.base_len,
            wal: dir.join("wal"),
            wal_len: held.wal_len,
        })
    }

    /// A readable copy of the held generation: the base beside the WAL
    /// under SQLite's own names, refreshed to the length held, so a
    /// read-only open replays it. The base is copied once and patched
    /// in place by every delta lay after, so a refresh rewrites only
    /// the WAL beside it, into a staged name renamed into place.
    /// [`None`] when nothing is held.
    ///
    /// # Errors
    /// Returns the io failure.
    pub async fn read_copy(&self, object_id: &str) -> Result<Option<PathBuf>, String> {
        let Some(mut meta) = self.lock(object_id, false).await else {
            return Ok(None);
        };
        let Some(held) = meta.held.as_mut() else {
            return Ok(None);
        };
        let dir = self.generation_dir(object_id, held.epoch, held.base);
        let copy = dir.join("read.db");
        if held.read_len != Some(held.wal_len) || !copy.exists() {
            let wal_len = held.wal_len as usize;
            let lay_base = held.read_len.is_none() || !copy.exists();
            let copy_for_write = copy.clone();
            let dir_for_write = dir.clone();
            tokio::task::spawn_blocking(move || -> Result<(), String> {
                if lay_base {
                    std::fs::copy(dir_for_write.join("base.db"), &copy_for_write)
                        .map_err(|e| e.to_string())?;
                }
                let mut wal =
                    std::fs::read(dir_for_write.join("wal")).map_err(|e| e.to_string())?;
                wal.truncate(wal_len);
                let wal_path = with_suffix(&copy_for_write, "-wal");
                let _ = std::fs::remove_file(with_suffix(&copy_for_write, "-shm"));
                if wal.is_empty() {
                    let _ = std::fs::remove_file(&wal_path);
                } else {
                    let staged = with_suffix(&copy_for_write, "-wal.next");
                    std::fs::write(&staged, wal).map_err(|e| e.to_string())?;
                    std::fs::rename(&staged, &wal_path).map_err(|e| e.to_string())?;
                }
                Ok(())
            })
            .await
            .map_err(|e| e.to_string())??;
            held.read_len = Some(held.wal_len);
        }
        Ok(Some(copy))
    }

    /// Drops copies the store covers entirely and nobody has appended
    /// to for the idle time. A copy is a cache of what the store holds;
    /// the next residency's first flight lays it again. A copy whose
    /// owner went quiet before saying how far the store covers it asks
    /// the store once per sweep, and a copy of an identity the store
    /// has forgotten leaves with its fence: a deletion is the one event
    /// that reaches the replicas by this path alone. An evicted copy
    /// otherwise keeps its fence, so an old owner's base cannot lay a
    /// generation the cluster already fenced off.
    pub async fn evict_idle(&self, coverage: &Coverage) {
        let candidates: Vec<String> = {
            let slots = self.slots.lock().await;
            slots
                .iter()
                .filter(|(_, slot)| {
                    slot.try_lock().is_ok_and(|meta| {
                        meta.held
                            .as_ref()
                            .is_some_and(|held| held.last_append.elapsed() >= self.idle)
                    })
                })
                .map(|(id, _)| id.clone())
                .collect()
        };
        for object_id in candidates {
            let Some(mut meta) = self.lock(&object_id, false).await else {
                continue;
            };
            // Re-checked under the lock: an append since the scan keeps
            // the copy, and nothing can land while the directory goes.
            let Some(held) = meta.held.clone() else {
                continue;
            };
            if held.last_append.elapsed() < self.idle {
                continue;
            }
            let cover = if held.covered >= held.wal_len {
                Cover::Through(held.wal_len)
            } else {
                coverage(object_id.clone(), held.epoch, held.base).await
            };
            let dir = match cover {
                Cover::Forgotten => self.object_dir(&object_id),
                Cover::Through(len) if len >= held.wal_len => {
                    self.generation_dir(&object_id, held.epoch, held.base)
                }
                Cover::Through(_) | Cover::Unknown => continue,
            };
            if tokio::fs::remove_dir_all(&dir).await.is_ok() {
                meta.held = None;
                self.gauges.objects_held.fetch_sub(1, Ordering::Relaxed);
                self.gauges
                    .bytes_held
                    .fetch_sub((held.wal_len + held.base_len) as i64, Ordering::Relaxed);
                if matches!(cover, Cover::Forgotten) {
                    // The slot goes with the directory; a later mention
                    // of the identity reads an empty directory afresh.
                    meta.fence = 0;
                    self.slots.lock().await.remove(&object_id);
                }
            }
        }
    }
}

/// What `gen.json` records beside a copy: the list its base was laid
/// from, so a delta can be checked against it after a restart.
#[derive(serde::Serialize, serde::Deserialize)]
struct GenerationRecord {
    base_len: u64,
    chunks: Vec<String>,
}

/// Rebuilds an object's state from its directory: the fence file and
/// the one generation directory, with the WAL's length as found. The
/// fence is never below the held generation's epoch, whatever the file
/// says, so a lost or truncated fence cannot let an older owner in. A
/// `rotating` marker means a lay was cut short: the copy is forgotten,
/// the fence kept.
fn load_meta(dir: &Path) -> Meta {
    let fence: u64 = std::fs::read_to_string(dir.join("fence"))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    let interrupted = dir.join("rotating").exists();
    let mut held: Option<Held> = None;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(generation) = name.to_str().and_then(parse_generation) else {
                continue;
            };
            if interrupted {
                let _ = std::fs::remove_dir_all(entry.path());
                continue;
            }
            let wal_len = std::fs::metadata(entry.path().join("wal"))
                .map(|m| m.len())
                .unwrap_or(0);
            let base_len = std::fs::metadata(entry.path().join("base.db"))
                .map(|m| m.len())
                .unwrap_or(0);
            let Some(record) = std::fs::read(entry.path().join("gen.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<GenerationRecord>(&bytes).ok())
                .filter(|record| record.base_len == base_len)
            else {
                // A copy whose list is missing or disagrees with its file
                // cannot take a delta and cannot be vouched for; gone.
                let _ = std::fs::remove_dir_all(entry.path());
                continue;
            };
            let candidate = Held {
                epoch: generation.0,
                base: generation.1,
                wal_len,
                base_len,
                chunks: record.chunks,
                covered: 0,
                read_len: None,
                last_append: std::time::Instant::now(),
            };
            // The newest generation wins; older ones are leftovers.
            if held
                .as_ref()
                .is_none_or(|h: &Held| (h.epoch, h.base) < generation)
            {
                held = Some(candidate);
            }
        }
    }
    if interrupted {
        let _ = std::fs::remove_file(dir.join("rotating"));
    }
    let fence = fence.max(held.as_ref().map_or(0, |h| h.epoch));
    Meta { fence, held }
}

fn parse_generation(name: &str) -> Option<(u64, u64)> {
    let rest = name.strip_prefix('e')?;
    let (epoch, base) = rest.split_once("-b")?;
    Some((epoch.parse().ok()?, base.parse().ok()?))
}

fn with_suffix(file: &Path, suffix: &str) -> PathBuf {
    let mut path = file.as_os_str().to_owned();
    path.push(suffix);
    PathBuf::from(path)
}

/// Group commit without a window: the loop syncs whatever has arrived
/// since its last round, each distinct file once, and answers them all.
/// A round takes as long as the disk takes, so under load the batch
/// grows on its own; idle, an append is synced the moment it arrives.
/// Every round opens its files afresh: a handle kept across rounds can
/// outlive the file it named (a generation replaced at the same path)
/// and sync nothing while reporting success.
async fn sync_loop(mut rx: mpsc::UnboundedReceiver<SyncRequest>) {
    while let Some(first) = rx.recv().await {
        let mut batch = vec![first];
        while let Ok(more) = rx.try_recv() {
            batch.push(more);
        }
        let paths: Vec<PathBuf> = {
            let mut seen = std::collections::HashSet::new();
            batch
                .iter()
                .filter(|r| seen.insert(r.path.clone()))
                .map(|r| r.path.clone())
                .collect()
        };
        let outcomes = tokio::task::spawn_blocking(move || {
            paths
                .into_iter()
                .map(|path| {
                    let result = std::fs::File::open(&path)
                        .and_then(|file| file.sync_data())
                        .map_err(|e| e.to_string());
                    (path, result)
                })
                .collect::<HashMap<PathBuf, Result<(), String>>>()
        })
        .await
        .unwrap_or_default();
        for request in batch {
            let result = outcomes
                .get(&request.path)
                .cloned()
                .unwrap_or_else(|| Err("the sync task died".to_owned()));
            let _ = request.done.send(result);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::store::{CHUNK_BYTES, list_hash};
    use actias_worker_core::proto::Bytes;

    fn store(idle: std::time::Duration) -> (Arc<ReplicaStore>, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        (
            ReplicaStore::new(dir.path().to_path_buf(), SyncMode::Os, idle),
            dir,
        )
    }

    fn no_coverage() -> Coverage {
        Arc::new(|_, _, _| Box::pin(async { Cover::Unknown }))
    }

    /// The chunk list of `bytes`, as an owner would compute it.
    fn chunks_of(bytes: &[u8]) -> Vec<String> {
        bytes
            .chunks(CHUNK_BYTES as usize)
            .map(|chunk| blake3::hash(chunk).to_hex().to_string())
            .collect()
    }

    /// Lays a whole base of `bytes` at the generation.
    async fn lay_full(
        store: &ReplicaStore,
        id: &str,
        epoch: u64,
        base: u64,
        bytes: &[u8],
    ) -> Appended {
        let chunks = chunks_of(bytes);
        let parts: Vec<LayPart> = bytes
            .chunks(CHUNK_BYTES as usize)
            .enumerate()
            .map(|(i, chunk)| Ok((i as u32, Bytes::copy_from_slice(chunk))))
            .collect();
        store
            .lay(
                id,
                LayHeader {
                    epoch,
                    base,
                    from_list: String::new(),
                    base_len: bytes.len() as u64,
                    chunks,
                },
                futures::stream::iter(parts),
            )
            .await
            .expect("lays")
    }

    #[tokio::test]
    async fn appends_are_idempotent_by_offset_and_gaps_are_refused() {
        let (store, _dir) = store(std::time::Duration::from_secs(1800));
        let started = lay_full(&store, "o", 3, 0, b"BASE").await;
        assert!(started.applied);

        let first = store.append("o", 3, 0, 0, b"abcd", 0).await.expect("ok");
        assert_eq!(first.length, 4);
        // The same bytes again: acked, nothing written twice.
        let again = store.append("o", 3, 0, 0, b"abcd", 0).await.expect("ok");
        assert_eq!((again.applied, again.length), (true, 4));
        // Overlapping: only the new tail lands.
        let overlap = store.append("o", 3, 0, 2, b"cdef", 0).await.expect("ok");
        assert_eq!((overlap.applied, overlap.length), (true, 6));
        // A gap says how much is held so the owner resends from there.
        let gap = store.append("o", 3, 0, 9, b"xyz", 0).await.expect("ok");
        assert!(!gap.applied);
        assert_eq!(gap.length, 6);
        assert_eq!(gap.refusal, refusal::GAP);

        let copy = store.fetch("o", 3, 0).await.expect("held");
        assert_eq!(std::fs::read(&copy.base).expect("base"), b"BASE");
        assert_eq!(copy.base_len, 4);
        let mut wal = std::fs::read(&copy.wal).expect("wal");
        wal.truncate(copy.wal_len as usize);
        assert_eq!(wal, b"abcdef");

        // The read copy carries base and WAL under SQLite's names, at
        // the held length.
        let copy = store.read_copy("o").await.expect("ok").expect("held");
        assert_eq!(std::fs::read(&copy).expect("base copy"), b"BASE");
        assert_eq!(
            std::fs::read(with_suffix(&copy, "-wal")).expect("wal copy"),
            b"abcdef"
        );
    }

    #[tokio::test]
    async fn the_fence_refuses_an_older_epoch_after_a_takeover_asked() {
        let (store, _dir) = store(std::time::Duration::from_secs(1800));
        lay_full(&store, "o", 3, 0, b"BASE").await;
        store.append("o", 3, 0, 0, b"abcd", 0).await.expect("ok");

        // The new owner at epoch 4 asks, raising the fence.
        let info = store.state("o", 3, 0, 4).await.expect("fence written");
        assert_eq!(
            info,
            Info {
                held: true,
                length: 4,
                fence: 4
            }
        );

        // The zombie at epoch 3 can no longer append, nor lay a base.
        let zombie = store.append("o", 3, 0, 4, b"ef", 0).await.expect("ok");
        assert!(!zombie.applied);
        assert_eq!(zombie.refusal, refusal::FENCED);
        assert_eq!(zombie.length, 4);
        let relay = lay_full(&store, "o", 3, 1, b"STALE").await;
        assert_eq!(relay.refusal, refusal::FENCED);
        assert!(
            store.state("o", 3, 0, 0).await.expect("ok").held,
            "the copy stands"
        );

        // A generation without its base is refused, asking for it.
        let missing = store.append("p", 1, 0, 0, b"ab", 0).await.expect("ok");
        assert!(!missing.applied);
        assert_eq!(missing.refusal, refusal::NO_BASE);
    }

    #[tokio::test]
    async fn a_delta_patches_the_base_and_the_read_copy_in_place() {
        let (store, dir) = store(std::time::Duration::from_secs(1800));
        let chunk = CHUNK_BYTES as usize;
        let mut first = vec![b'a'; chunk];
        first.extend(vec![b'b'; chunk]);
        first.extend(vec![b'c'; 10]);
        lay_full(&store, "o", 1, 0, &first).await;
        store.append("o", 1, 0, 0, b"wal", 0).await.expect("ok");
        // A read copy exists, so the delta must keep it current too.
        let read = store.read_copy("o").await.expect("ok").expect("held");
        assert_eq!(std::fs::read(&read).expect("read copy"), first);

        // Rotation: the middle chunk changed and the file grew a byte.
        let mut second = first.clone();
        second[chunk..2 * chunk].fill(b'B');
        second.push(b'd');
        let chunks = chunks_of(&second);
        let parts: Vec<LayPart> = vec![
            Ok((1, Bytes::copy_from_slice(&second[chunk..2 * chunk]))),
            Ok((2, Bytes::copy_from_slice(&second[2 * chunk..]))),
        ];
        let delta = store
            .lay(
                "o",
                LayHeader {
                    epoch: 1,
                    base: 1,
                    from_list: list_hash(&chunks_of(&first)),
                    base_len: second.len() as u64,
                    chunks: chunks.clone(),
                },
                futures::stream::iter(parts),
            )
            .await
            .expect("lays");
        assert!(delta.applied, "{}", delta.refusal);
        let copy = store
            .fetch("o", 1, 1)
            .await
            .expect("held at the new generation");
        assert_eq!(std::fs::read(&copy.base).expect("base"), second);
        assert_eq!(copy.wal_len, 0, "the WAL restarted");
        assert!(
            store.fetch("o", 1, 0).await.is_none(),
            "the old generation is gone"
        );
        assert!(!dir.path().join("o").join("rotating").exists());
        // The read copy was patched alongside, WAL dropped.
        let read = store.read_copy("o").await.expect("ok").expect("held");
        assert_eq!(std::fs::read(&read).expect("read copy"), second);
        assert!(!with_suffix(&read, "-wal").exists());

        // A delta over a list this copy is not on asks for everything.
        let off = store
            .lay(
                "o",
                LayHeader {
                    epoch: 1,
                    base: 2,
                    from_list: "not-this-list".to_owned(),
                    base_len: 1,
                    chunks: chunks_of(b"z"),
                },
                futures::stream::iter(Vec::<LayPart>::new()),
            )
            .await
            .expect("answers");
        assert_eq!(off.refusal, refusal::NO_BASE);
        assert!(store.fetch("o", 1, 1).await.is_some(), "the copy stands");

        // A lay cut short forgets the copy rather than keeping half of it.
        let cut = store
            .lay(
                "o",
                LayHeader {
                    epoch: 1,
                    base: 2,
                    from_list: list_hash(&chunks),
                    base_len: second.len() as u64,
                    chunks: chunks.clone(),
                },
                futures::stream::iter(vec![Err::<(u32, Bytes), String>("cut".to_owned())]),
            )
            .await;
        assert!(cut.is_err());
        assert!(store.fetch("o", 1, 2).await.is_none());
        assert!(store.fetch("o", 1, 1).await.is_none());
        let again = lay_full(&store, "o", 1, 3, b"fresh").await;
        assert!(again.applied);
    }

    #[tokio::test]
    async fn a_restart_reads_the_copy_and_the_fence_back_from_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        {
            let store = ReplicaStore::new(
                dir.path().to_path_buf(),
                SyncMode::Fsync,
                std::time::Duration::from_secs(1800),
            );
            lay_full(&store, "o", 2, 1, b"BASE").await;
            store.append("o", 2, 1, 0, b"abcd", 0).await.expect("ok");
            store.state("o", 2, 1, 5).await.expect("fence written");
        }
        // A lost fence file cannot let an owner older than the copy in.
        std::fs::remove_file(dir.path().join("o").join("fence")).expect("fence removed");
        let store = ReplicaStore::new(
            dir.path().to_path_buf(),
            SyncMode::Fsync,
            std::time::Duration::from_secs(1800),
        );
        let info = store.state("o", 2, 1, 0).await.expect("ok");
        assert_eq!(
            info,
            Info {
                held: true,
                length: 4,
                fence: 2
            }
        );
        let copy = store.fetch("o", 2, 1).await.expect("held");
        assert_eq!(std::fs::read(&copy.wal).expect("wal"), b"abcd");
        let older = lay_full(&store, "o", 1, 0, b"OLD").await;
        assert_eq!(older.refusal, refusal::FENCED);
        // The list survived the restart, so a delta still applies.
        let delta = store
            .lay(
                "o",
                LayHeader {
                    epoch: 2,
                    base: 2,
                    from_list: list_hash(&chunks_of(b"BASE")),
                    base_len: 4,
                    chunks: chunks_of(b"BASF"),
                },
                futures::stream::iter(vec![Ok::<_, String>((0u32, Bytes::from_static(b"BASF")))]),
            )
            .await
            .expect("lays");
        assert!(delta.applied);

        // A marker left by a lay that died mid-patch forgets the copy.
        std::fs::write(dir.path().join("o").join("rotating"), b"").expect("marker");
        let store = ReplicaStore::new(
            dir.path().to_path_buf(),
            SyncMode::Fsync,
            std::time::Duration::from_secs(1800),
        );
        let info = store.state("o", 2, 2, 0).await.expect("ok");
        assert_eq!(
            info,
            Info {
                held: false,
                length: 0,
                fence: 2
            },
            "forgotten, fence kept"
        );
    }

    #[tokio::test]
    async fn a_covered_idle_copy_leaves_and_an_uncovered_one_stays() {
        let (store, _dir) = store(std::time::Duration::from_millis(0));
        lay_full(&store, "a", 1, 0, b"B").await;
        store.append("a", 1, 0, 0, b"abcd", 0).await.expect("ok");
        lay_full(&store, "b", 1, 0, b"B").await;
        // The owner's next append says the store covers 4 bytes of "b".
        store.append("b", 1, 0, 0, b"abcd", 4).await.expect("ok");
        lay_full(&store, "c", 1, 0, b"B").await;
        store.append("c", 1, 0, 0, b"abcd", 0).await.expect("ok");

        // "a": nobody said; the store, asked, holds nothing. "c": the
        // store, asked, covers it.
        let coverage: Coverage = Arc::new(|id, _, _| {
            Box::pin(async move {
                if id == "c" {
                    Cover::Through(4)
                } else {
                    Cover::Unknown
                }
            })
        });
        store.evict_idle(&coverage).await;
        assert!(
            store.state("a", 1, 0, 0).await.expect("ok").held,
            "uncovered: stays"
        );
        assert!(
            !store.state("b", 1, 0, 0).await.expect("ok").held,
            "covered and idle: gone"
        );
        assert!(
            !store.state("c", 1, 0, 0).await.expect("ok").held,
            "the store covers it: gone"
        );
        let _ = no_coverage();
    }

    #[tokio::test]
    async fn a_forgotten_identity_leaves_with_its_fence() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ReplicaStore::new(
            dir.path().to_path_buf(),
            SyncMode::Os,
            std::time::Duration::ZERO,
        );
        lay_full(&store, "gone", 3, 0, b"B").await;
        store.append("gone", 3, 0, 0, b"ab", 0).await.expect("ok");
        assert!(dir.path().join("gone").join("fence").exists());

        let forgotten: Coverage = Arc::new(|_, _, _| Box::pin(async { Cover::Forgotten }));
        store.evict_idle(&forgotten).await;
        assert!(
            !dir.path().join("gone").exists(),
            "directory and fence gone"
        );
        assert_eq!(
            store.state("gone", 3, 0, 0).await.expect("ok"),
            Info {
                held: false,
                length: 0,
                fence: 0
            },
            "nothing remembered"
        );
        // A recreation at any epoch lays a fresh base.
        let again = lay_full(&store, "gone", 1, 0, b"NEW").await;
        assert!(again.applied);
    }
}
