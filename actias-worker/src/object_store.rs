//! Snapshot shipping: an object's durable truth leaves the node. Small
//! objects ship whole (a consistent sqlite snapshot per flight); past a
//! size threshold the flight ships only the WAL frames committed since
//! the last one, against a base laid down at a checkpoint, litestream's
//! model natively (docs/WAL-SHIPPING.md). The manifest carries the lease
//! epoch as the fence either way, and restore-on-spawn brings the object
//! back wherever it is next resident, which makes failover and rehoming
//! the same code path.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// What the store remembers about one object's shipped state.
///
/// Version 1 manifests (`{epoch, shipped_at}` beside a `snapshot.db`)
/// still read; everything shipped now is version 2, where the state
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
}

fn one() -> u32 {
    1
}

/// Per-residency shipping state, owned by the object's ship closure. A
/// fresh residency has a fresh epoch, so it always starts by laying a
/// new generation; nothing needs recovering from the store.
#[derive(Default)]
pub struct ShipState {
    mode: Option<Mode>,
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
}

impl ObjectStore {
    pub fn new(client: aws_sdk_s3::Client, bucket: String) -> Self {
        Self { client, bucket }
    }

    /// Version 1's whole-database key, read-only now.
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
    /// on. Any segment-path failure falls back to a whole flight, so
    /// shipping never regresses below today's behavior.
    pub async fn ship(
        &self,
        object_id: &str,
        epoch: u64,
        file: &Path,
        state: &tokio::sync::Mutex<ShipState>,
        whole_max: u64,
    ) -> Result<(), String> {
        if let Some(manifest) = self.manifest(object_id).await?
            && manifest.epoch > epoch
        {
            return Err(format!(
                "fenced: epoch {epoch} lost to {}; this node no longer owns the object",
                manifest.epoch
            ));
        }

        let mut state = state.lock().await;
        let db_len = file.metadata().map(|m| m.len()).unwrap_or(0);
        let wal_len = wal_path(file).metadata().map(|m| m.len()).unwrap_or(0);

        match state.mode {
            Some(Mode::Segments {
                base,
                segments,
                salts,
                offset,
            }) => {
                match self
                    .segment_flight(object_id, epoch, file, base, segments, salts, offset)
                    .await
                {
                    Ok(next) => {
                        state.mode = Some(next);
                        Ok(())
                    }
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
                        Ok(())
                    }
                }
            }
            _ if db_len + wal_len >= whole_max => {
                let base = match state.mode {
                    Some(Mode::Whole { base }) => base + 1,
                    _ => 0,
                };
                match self.start_generation(object_id, epoch, file, base).await {
                    Ok(next) => {
                        state.mode = Some(next);
                        Ok(())
                    }
                    Err(error) => {
                        actias_common::tracing::warn!(
                            object_id,
                            %error,
                            "generation start failed; shipping whole"
                        );
                        state.mode = Some(self.whole_flight(object_id, epoch, file, base).await?);
                        Ok(())
                    }
                }
            }
            _ => {
                let base = match state.mode {
                    Some(Mode::Whole { base }) => base,
                    _ => 0,
                };
                state.mode = Some(self.whole_flight(object_id, epoch, file, base).await?);
                Ok(())
            }
        }
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
        let bytes = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
            let tmp = source.with_extension("ship");
            let mut storage = actias_worker_core::storage::SqliteStorage::open(&source)?;
            storage.snapshot_to(&tmp)?;
            let bytes = std::fs::read(&tmp).map_err(|e| e.to_string())?;
            let _ = std::fs::remove_file(&tmp);
            let _ = storage.checkpoint_passive();
            Ok(bytes)
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
        let bytes = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
            let mut storage = actias_worker_core::storage::SqliteStorage::open(&source)?;
            storage.checkpoint()?;
            std::fs::read(&source).map_err(|e| e.to_string())
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

    /// Restores the last shipped state into `file`; false when nothing
    /// was ever shipped (a genuinely new object).
    pub async fn restore(&self, object_id: &str, file: &Path) -> Result<bool, String> {
        let Some(manifest) = self.manifest(object_id).await? else {
            return Ok(false);
        };

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
