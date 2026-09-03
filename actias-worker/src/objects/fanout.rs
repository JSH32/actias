//! The owner side of tail replication: which nodes replicate an object,
//! and the fan-out that sends every flight's frames to them and counts
//! the acks the gate may answer on.

use std::path::PathBuf;
use std::sync::Arc;

use crate::data_plane::{authed, peer_client};
use crate::objects::store::{FanoutFn, FanoutOutcome, FanoutRequest};
use crate::server::AppState;
use actias_worker_core::proto::Bytes;
use actias_worker_core::proto::worker_data::WalAppend;

/// The replica set for one residency: the top `replica_count` live
/// nodes other than this one by rendezvous rank over the object id, so
/// two owners of the same object in succession mostly agree and a node
/// leaving moves only the objects it replicated.
pub async fn choose_replicas(state: &AppState, object_id: &str) -> Vec<String> {
    if state.replica_count == 0 {
        return Vec::new();
    }
    let own = state
        .node_identity
        .read()
        .ok()
        .and_then(|guard| guard.clone());
    let mut ranked: Vec<(String, String)> = Vec::new();
    for node in crate::directory::rebuild::live_nodes(state).await {
        if own.as_deref() == Some(node.as_str()) {
            continue;
        }
        // A registration this node left behind (a restart before the
        // registry reaped it) answers at our own address; a copy there
        // is no copy at all.
        if crate::directory::route::address_of(state, &node)
            .await
            .is_ok_and(|address| address == state.node_address)
        {
            continue;
        }
        let rank = blake3::hash(format!("replica:{object_id}:{node}").as_bytes())
            .to_hex()
            .to_string();
        ranked.push((rank, node));
    }
    ranked.sort();
    ranked.reverse();
    ranked
        .into_iter()
        .take(state.replica_count)
        .map(|(_, node)| node)
        .collect()
}

enum Sent {
    Acked,
    Fenced,
    Failed,
}

/// Builds the fan-out for one residency: every request goes to every
/// replica in parallel, a replica behind is resent from the length it
/// reports, one lacking the generation is sent the base, and a fence
/// from any of them marks the flight as lost to a newer owner.
pub fn fanout_for(
    state: AppState,
    object_id: String,
    file: PathBuf,
    replicas: Vec<String>,
) -> FanoutFn {
    Arc::new(move |request: FanoutRequest| {
        let state = state.clone();
        let object_id = object_id.clone();
        let file = file.clone();
        let replicas = replicas.clone();
        let request = Arc::new(request);
        Box::pin(async move {
            use futures::StreamExt;
            let quorum = request.quorum;
            let mut sends: futures::stream::FuturesUnordered<_> = replicas
                .iter()
                .map(|node| {
                    append_to(
                        state.clone(),
                        node.clone(),
                        object_id.clone(),
                        file.clone(),
                        request.clone(),
                    )
                })
                .collect();
            let mut acks = 0;
            let mut fenced = false;
            // The quorum answers as soon as it exists; the stragglers get
            // a short grace so a dead replica never holds the flight to
            // its whole budget. They are resent from their length next
            // flight.
            let mut grace: Option<std::pin::Pin<Box<tokio::time::Sleep>>> = None;
            loop {
                let next = tokio::select! {
                    outcome = sends.next() => outcome,
                    _ = async {
                        match grace.as_mut() {
                            Some(sleep) => sleep.await,
                            None => std::future::pending().await,
                        }
                    } => None,
                };
                match next {
                    Some(Sent::Acked) => acks += 1,
                    Some(Sent::Fenced) => fenced = true,
                    Some(Sent::Failed) => {}
                    None => break,
                }
                if quorum > 0 && acks >= quorum && grace.is_none() {
                    grace = Some(Box::pin(tokio::time::sleep(STRAGGLER_GRACE)));
                }
            }
            FanoutOutcome { acks, fenced }
        })
    })
}

/// Longest exchange with one replica for one flight: the append, and
/// at most two corrections (a base, a resend from its length).
const ATTEMPTS: usize = 3;

/// How long a flight waits for the replicas past its quorum before it
/// goes on without them.
const STRAGGLER_GRACE: std::time::Duration = std::time::Duration::from_millis(20);

async fn append_to(
    state: AppState,
    node: String,
    object_id: String,
    file: PathBuf,
    request: Arc<FanoutRequest>,
) -> Sent {
    let Ok(address) = crate::directory::route::address_of(&state, &node).await else {
        return Sent::Failed;
    };
    let Ok(mut client) = peer_client(&state, &address).await else {
        return Sent::Failed;
    };

    let mut offset = request.offset as usize;
    let mut base_bytes: Option<Bytes> = request.base_bytes.clone();
    for _ in 0..ATTEMPTS {
        let bytes = request.wal.slice(offset.min(request.wal.len())..);
        let append = WalAppend {
            object_id: object_id.clone(),
            epoch: request.epoch,
            base: request.base,
            offset: offset as u64,
            bytes,
            base_bytes: base_bytes.take(),
            covered: request.covered,
        };
        let reply = tokio::time::timeout(
            state.replica_ack,
            client.append_wal(authed(&state.internal_token, append)),
        )
        .await;
        let reply = match reply {
            Ok(Ok(reply)) => reply.into_inner(),
            Ok(Err(status)) => {
                actias_common::tracing::warn!(
                    object_id,
                    node,
                    error = %status,
                    "replica append failed"
                );
                return Sent::Failed;
            }
            Err(_) => {
                actias_common::tracing::warn!(object_id, node, "replica append timed out");
                return Sent::Failed;
            }
        };
        if reply.applied {
            return Sent::Acked;
        }
        if reply.refusal == crate::objects::replica::refusal::FENCED {
            actias_common::tracing::warn!(
                object_id,
                node,
                refusal = reply.refusal,
                "replica fenced this owner"
            );
            return Sent::Fenced;
        }
        if reply.refusal == crate::objects::replica::refusal::NO_BASE {
            // The replica lacks the generation: the main file is its base,
            // unchanged since the generation's checkpoint because only the
            // shipper checkpoints and its flights never overlap.
            let path = file.clone();
            match tokio::task::spawn_blocking(move || std::fs::read(&path)).await {
                Ok(Ok(bytes)) => {
                    base_bytes = Some(Bytes::from(bytes));
                    offset = 0;
                }
                _ => return Sent::Failed,
            }
        } else if reply.refusal == crate::objects::replica::refusal::GAP {
            offset = reply.length as usize;
        } else {
            return Sent::Failed;
        }
    }
    Sent::Failed
}
