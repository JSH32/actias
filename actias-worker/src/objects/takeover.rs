//! Takeover from replicas: the new lease holder lays the object's file
//! from the freshest replica copy instead of restoring from the store.
//! It asks every replica the manifest names for the generation the
//! manifest names, raising their fences to the new epoch as it goes,
//! and once a quorum has answered it takes the longest valid copy. With
//! 2Q > K any two quorums intersect, so at least one replica that acked
//! a returned write is among those read, and the longest copy holds it.

use std::path::Path;
use std::sync::atomic::Ordering;

use crate::data_plane::{authed, peer_client};
use crate::objects::store::Manifest;
use crate::server::AppState;
use actias_worker_core::proto::worker_data::ReplicaQuery;

/// One replica's answer, kept for the pick.
struct Answer {
    node_id: String,
    address: String,
    length: u64,
}

/// Lays `file` from a replica when a quorum of the manifest's replicas
/// holds a copy at least as long as the store's. Answers `false` when
/// the store must be used instead: no replicas named, shadow mode, too
/// few answers (an incident), or the store is ahead.
///
/// # Errors
/// Returns the failure of laying the file once a replica was chosen;
/// the caller falls back to the store.
pub async fn from_replicas(
    state: &AppState,
    object_id: &str,
    manifest: &Manifest,
    new_epoch: u64,
    file: &Path,
) -> Result<bool, String> {
    if manifest.replicas.is_empty() || state.replica_quorum == 0 {
        return Ok(false);
    }
    let quorum = state.replica_quorum.min(manifest.replicas.len());

    // This node's own copy counts as one answer, whatever id it held
    // it under: a node that restarted has a new id the manifest does
    // not name, but the same bytes on disk. It is fenced like any
    // replica and never trusted alone; the quorum decides.
    let own_copy = state
        .replica_store
        .state(object_id, manifest.epoch, manifest.base, new_epoch)
        .await
        .ok()
        .filter(|info| info.held)
        .map(|info| info.length);

    let asks = manifest.replicas.iter().map(|node_id| {
        let node_id = node_id.clone();
        async move {
            let address = crate::directory::route::address_of(state, &node_id)
                .await
                .ok()?;
            let mut client = peer_client(state, &address).await.ok()?;
            let query = ReplicaQuery {
                object_id: object_id.to_owned(),
                epoch: manifest.epoch,
                base: manifest.base,
                fence_to: new_epoch,
            };
            let info = tokio::time::timeout(
                state.replica_ack,
                client.replica_state(authed(&state.internal_token, query)),
            )
            .await
            .ok()?
            .ok()?
            .into_inner();
            info.held.then_some(Answer {
                node_id,
                address,
                length: info.length,
            })
        }
    });
    let mut answers: Vec<Answer> = futures::future::join_all(asks)
        .await
        .into_iter()
        .flatten()
        .collect();
    if let Some(length) = own_copy {
        answers.push(Answer {
            node_id: "this node".to_owned(),
            address: String::new(),
            length,
        });
    }

    if answers.len() < quorum {
        state
            .replica_store
            .gauges
            .takeover_incidents
            .fetch_add(1, Ordering::Relaxed);
        actias_common::tracing::warn!(
            object_id,
            epoch = manifest.epoch,
            answered = answers.len(),
            quorum,
            named = ?manifest.replicas,
            "durability incident: fewer replicas than the quorum answered; restoring from the store"
        );
        return Ok(false);
    }
    let Some(best) = answers.iter().max_by_key(|a| a.length) else {
        return Ok(false);
    };
    if best.length < manifest.wal_len {
        // Every acked write is on the longest copy, so this only happens
        // when the store outran the replicas on a flight that missed its
        // quorum; the store is then the fresher copy.
        actias_common::tracing::info!(
            object_id,
            replica = best.node_id,
            held = best.length,
            shipped = manifest.wal_len,
            "the store is ahead of the replicas; restoring from it"
        );
        return Ok(false);
    }

    let (base, wal) = if best.address.is_empty() {
        state
            .replica_store
            .fetch(object_id, manifest.epoch, manifest.base)
            .await?
            .ok_or_else(|| "this node's copy vanished under the takeover".to_owned())?
    } else {
        let mut client = peer_client(state, &best.address).await?;
        let query = ReplicaQuery {
            object_id: object_id.to_owned(),
            epoch: manifest.epoch,
            base: manifest.base,
            fence_to: new_epoch,
        };
        let mut stream = client
            .fetch_replica(authed(&state.internal_token, query))
            .await
            .map_err(|status| status.message().to_owned())?
            .into_inner();
        let mut base = Vec::new();
        let mut wal = Vec::new();
        while let Some(chunk) = stream
            .message()
            .await
            .map_err(|status| status.message().to_owned())?
        {
            base.extend_from_slice(&chunk.base);
            wal.extend_from_slice(&chunk.wal);
        }
        (base, wal)
    };
    if base.is_empty() {
        return Err("the replica handed over an empty base".to_owned());
    }
    lay(file, base, wal).await?;
    state
        .replica_store
        .gauges
        .takeovers
        .fetch_add(1, Ordering::Relaxed);
    actias_common::tracing::info!(
        object_id,
        replica = best.node_id,
        held = best.length,
        "object taken over from a replica"
    );
    Ok(true)
}

/// Lays base and WAL the way restore does: sidecars first, so a stale
/// local WAL is never recovered into the new base, then base and WAL,
/// then one checkpoint to fold the WAL in.
async fn lay(file: &Path, base: Vec<u8>, wal: Vec<u8>) -> Result<(), String> {
    let wal_path = with_suffix(file, "-wal");
    let _ = tokio::fs::remove_file(&wal_path).await;
    let _ = tokio::fs::remove_file(with_suffix(file, "-shm")).await;
    tokio::fs::write(file, base)
        .await
        .map_err(|e| e.to_string())?;
    if !wal.is_empty() {
        tokio::fs::write(&wal_path, wal)
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
    Ok(())
}

fn with_suffix(file: &Path, suffix: &str) -> std::path::PathBuf {
    let mut path = file.as_os_str().to_owned();
    path.push(suffix);
    std::path::PathBuf::from(path)
}
