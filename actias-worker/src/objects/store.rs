//! Snapshot shipping: an object's durable truth leaves the node. Small
//! objects ship whole (a consistent sqlite snapshot per flight); past a
//! size threshold the flight ships only the WAL frames committed since
//! the last one, against a base laid down at a checkpoint, litestream's
//! model natively. The manifest carries the lease
//! epoch as the fence either way, and restore-on-spawn brings the object
//! back wherever it is next resident, which makes failover and rehoming
//! the same code path.

use actias_worker_core::directory::manifest::Manifest as DirectoryManifest;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// What the store remembers about one object's shipped state.
///
/// Version 1 manifests (`{epoch, shipped_at}` beside a `snapshot.db`)
/// still read; everything ships as version 2, where the state
/// lives under a generation directory `e{epoch}-b{base}` as a `base.db`
/// plus zero or more `wal-{n:05}.seg` slices of one live WAL.
#[derive(Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default = "one")]
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
    /// True when `base.db` is a complete database and segments are moot.
    #[serde(default)]
    pub whole: bool,
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
}

fn one() -> u32 {
    1
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
    mode: Option<Mode>,
}

/// Operator-tunable shipping sizes, one copy per node.
#[derive(Clone, Copy)]
pub struct ShipThresholds {
    /// Databases at or past this size ship WAL segments, not the file.
    pub whole_max: u64,
    /// A WAL this large rotates the generation at the next flight.
    pub rotate_bytes: u64,
    /// So does this many segments, whatever their size.
    pub max_segments: u32,
}

enum Mode {
    Whole {
        base: u64,
    },
    Segments {
        base: u64,
        segments: u32,
        /// The WAL incarnation the offsets count against; learned from
        /// the first frames after the generation's checkpoint.
        salts: Option<(u32, u32)>,
        /// Shipped through this byte of the WAL file.
        offset: usize,
    },
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

    /// Version 1's whole-database key; restore reads it, nothing
    /// writes it.
    fn snapshot_key(object_id: &str) -> String {
        format!("objects/{object_id}/snapshot.db")
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
    /// object now, and this upload is refused.
    ///
    /// Small databases ship whole; at `whole_max` bytes the flight lays
    /// a base at a checkpoint and ships committed WAL frames from then
    /// on, rotating the generation when the WAL or the segment count
    /// grows past the thresholds. Any segment-path failure falls back
    /// to a whole flight, so shipping never regresses below today's
    /// behavior. A flight that lays a new generation sweeps old ones,
    /// keeping the newest previous as the rollback margin.
    pub async fn ship(
        &self,
        object_id: &str,
        epoch: u64,
        file: &Path,
        state: &tokio::sync::Mutex<ShipState>,
        thresholds: ShipThresholds,
    ) -> Result<(), String> {
        if let Some(manifest) = self.manifest(object_id).await?
            && (manifest.epoch > epoch || (manifest.deleted && manifest.epoch >= epoch))
        {
            return Err(format!(
                "fenced: epoch {epoch} lost to {}; this node no longer owns the object",
                manifest.epoch
            ));
        }

        let mut state = state.lock().await;
        let db_len = file.metadata().map(|m| m.len()).unwrap_or(0);
        let wal_len = wal_path(file).metadata().map(|m| m.len()).unwrap_or(0);
        let base_before = state.mode.as_ref().map(|mode| match *mode {
            Mode::Whole { base } | Mode::Segments { base, .. } => base,
        });

        match state.mode {
            Some(Mode::Segments {
                base,
                segments,
                salts,
                offset,
            }) => {
                if segments >= thresholds.max_segments || wal_len >= thresholds.rotate_bytes {
                    // Rotation: the checkpoint folds every frame, shipped
                    // or not, into the next base; nothing can be lost to
                    // the boundary.
                    match self
                        .start_generation(object_id, epoch, file, base + 1)
                        .await
                    {
                        Ok(next) => state.mode = Some(next),
                        Err(error) => {
                            actias_common::tracing::warn!(
                                object_id,
                                %error,
                                "rotation failed; shipping whole"
                            );
                            state.mode =
                                Some(self.whole_flight(object_id, epoch, file, base + 1).await?);
                        }
                    }
                } else {
                    match self
                        .segment_flight(object_id, epoch, file, base, segments, salts, offset)
                        .await
                    {
                        Ok(next) => state.mode = Some(next),
                        Err(error) => {
                            // The fallback that keeps the promise: a whole
                            // flight ships everything the segment could not.
                            actias_common::tracing::warn!(
                                object_id,
                                %error,
                                "segment flight failed; shipping whole"
                            );
                            state.mode =
                                Some(self.whole_flight(object_id, epoch, file, base + 1).await?);
                        }
                    }
                }
            }
            _ if db_len + wal_len >= thresholds.whole_max => {
                let base = match state.mode {
                    Some(Mode::Whole { base }) => base + 1,
                    _ => 0,
                };
                match self.start_generation(object_id, epoch, file, base).await {
                    Ok(next) => state.mode = Some(next),
                    Err(error) => {
                        actias_common::tracing::warn!(
                            object_id,
                            %error,
                            "generation start failed; shipping whole"
                        );
                        state.mode = Some(self.whole_flight(object_id, epoch, file, base).await?);
                    }
                }
            }
            _ => {
                let base = match state.mode {
                    Some(Mode::Whole { base }) => base,
                    _ => 0,
                };
                state.mode = Some(self.whole_flight(object_id, epoch, file, base).await?);
            }
        }

        // A new generation retires old ones; failures only defer the
        // sweep to the next rotation.
        let base_after = state.mode.as_ref().map(|mode| match *mode {
            Mode::Whole { base } | Mode::Segments { base, .. } => base,
        });
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
    /// the current one is young), and the manifest. The legacy
    /// `snapshot.db` counts as the oldest generation, so it ages out
    /// the same way.
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
                if rest == "snapshot.db" {
                    generations.entry((0, 0)).or_default().push(key.to_owned());
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

    /// The whole-database flight. The snapshot goes through sqlite,
    /// never a raw byte copy: the object's task may be writing and the
    /// WAL may hold committed frames the main file lacks; VACUUM INTO
    /// sees all of it, consistently. The passive checkpoint afterwards
    /// bounds WAL growth without waiting on anyone.
    async fn whole_flight(
        &self,
        object_id: &str,
        epoch: u64,
        file: &Path,
        base: u64,
    ) -> Result<Mode, String> {
        let source = file.to_path_buf();
        // The directory row rides this pass: the connection is already
        // open, so carrying the row costs one point read rather than a
        // second flight.
        let (bytes, directory) =
            tokio::task::spawn_blocking(move || -> Result<(Vec<u8>, DirectoryRow), String> {
                let tmp = source.with_extension("ship");
                let mut storage = actias_worker_core::storage::SqliteStorage::open(&source)?;
                storage.snapshot_to(&tmp)?;
                let bytes = std::fs::read(&tmp).map_err(|e| e.to_string())?;
                let _ = std::fs::remove_file(&tmp);
                let directory = read_directory_row(&mut storage);
                let _ = storage.checkpoint_passive();
                Ok((bytes, directory))
            })
            .await
            .map_err(|e| e.to_string())??;

        self.put(Self::base_key(object_id, epoch, base), bytes)
            .await?;
        self.put_manifest(
            object_id,
            &Manifest {
                version: 2,
                epoch,
                shipped_at: actias_worker_core::extensions::objects::unix_now_ms(),
                base,
                segments: 0,
                whole: true,
                deleted: false,
                directory,
            },
        )
        .await?;
        Ok(Mode::Whole { base })
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
    ) -> Result<Mode, String> {
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

        self.put(Self::base_key(object_id, epoch, base), bytes)
            .await?;
        self.put_manifest(
            object_id,
            &Manifest {
                version: 2,
                epoch,
                shipped_at: actias_worker_core::extensions::objects::unix_now_ms(),
                base,
                segments: 0,
                whole: false,
                deleted: false,
                directory,
            },
        )
        .await?;
        Ok(Mode::Segments {
            base,
            segments: 0,
            salts: None,
            offset: 0,
        })
    }

    /// Ships the WAL frames committed since the last flight as one
    /// segment. Anything surprising (a restarted WAL, an unreadable
    /// prefix) is an error; the caller falls back to a whole flight.
    #[allow(clippy::too_many_arguments)]
    async fn segment_flight(
        &self,
        object_id: &str,
        epoch: u64,
        file: &Path,
        base: u64,
        segments: u32,
        salts: Option<(u32, u32)>,
        offset: usize,
    ) -> Result<Mode, String> {
        let wal = match tokio::fs::read(wal_path(file)).await {
            Ok(bytes) => bytes,
            // No WAL file means nothing committed since the checkpoint.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error.to_string()),
        };
        if wal.len() < 32 {
            // Truncated or empty: nothing new since the generation began.
            return Ok(Mode::Segments {
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
            return Ok(Mode::Segments {
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

        self.put(
            Self::segment_key(object_id, epoch, base, segments),
            wal[offset..prefix.len].to_vec(),
        )
        .await?;
        self.put_manifest(
            object_id,
            &Manifest {
                version: 2,
                epoch,
                shipped_at: actias_worker_core::extensions::objects::unix_now_ms(),
                base,
                segments: segments + 1,
                whole: false,
                deleted: false,
                directory,
            },
        )
        .await?;
        Ok(Mode::Segments {
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
                whole: false,
                deleted: true,
                // A forgotten identity has no row; the directory's own
                // tombstone is what removes it from the class.
                directory: None,
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

        let base_key = if manifest.version < 2 {
            Self::snapshot_key(object_id)
        } else {
            Self::base_key(object_id, manifest.epoch, manifest.base)
        };
        let base = self.get(base_key).await?;
        tokio::fs::write(file, base)
            .await
            .map_err(|e| e.to_string())?;

        if manifest.version >= 2 && !manifest.whole && manifest.segments > 0 {
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
        let old: Manifest =
            serde_json::from_str(r#"{ "epoch": 5, "shipped_at": 1 }"#).expect("v1 parses");
        assert_eq!(old.version, 1);
        assert!(!old.deleted, "absence of the field means live data");

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
            whole: true,
            deleted: false,
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
