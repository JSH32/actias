//! The owner side of tail replication: which nodes replicate an object,
//! and the fan-out that sends every flight's frames, or a generation's
//! chunks, to them and counts the acks the gate may answer on.

use std::path::PathBuf;
use std::sync::Arc;

use crate::data_plane::{authed, peer_client};
use crate::objects::replica::refusal;
use crate::objects::store::{FanoutFn, FanoutOutcome, FanoutRequest, Payload};
use crate::server::AppState;
use actias_worker_core::proto::Bytes;
use actias_worker_core::proto::worker_data::{
    GenerationChunk, GenerationDone, GenerationHeader, GenerationPart, WalAppend, WalAppended,
    generation_part,
};

/// The replica set for one residency: the top `replica_count` live
/// nodes other than this one by rendezvous rank over the object id, so
/// two owners of the same object in succession mostly agree and a node
/// leaving moves only the objects it replicated.
pub async fn choose_replicas(state: &AppState, object_id: &str) -> Vec<String> {
    if state.replica_count == 0 {
        return Vec::new();
    }
    ranked(state, object_id)
        .await
        .into_iter()
        .take(state.replica_count)
        .collect()
}

enum Sent {
    Acked,
    Fenced,
    /// The replica did not take the flight; which one, so a dead one
    /// can be replaced.
    Failed(String),
}

/// A residency's replica set, shared by its fan-out and the manifests
/// that record it: a member that died leaves it, the next live node in
/// rendezvous order joins when the set is short, and the next flight
/// lays the generation on a newcomer.
pub type ReplicaSet = Arc<std::sync::Mutex<Vec<String>>>;

/// Every live node other than this one, best rank first, for
/// `object_id`; [`choose_replicas`] takes the head of it. Membership is
/// the reader snapshot (`AppState::reader_membership`), so ranking
/// costs no registry read.
async fn ranked(state: &AppState, object_id: &str) -> Vec<String> {
    let own = state
        .node_identity
        .read()
        .ok()
        .and_then(|guard| guard.clone());
    let mut ranked: Vec<(String, String)> = Vec::new();
    for node in crate::directory::route::live_nodes_cached(state)
        .await
        .iter()
    {
        if own.as_deref() == Some(node.as_str()) {
            continue;
        }
        // A registration this node left behind (a restart before the
        // registry reaped it) answers at our own address; a copy there
        // is no copy at all.
        if crate::directory::route::address_of(state, node)
            .await
            .is_ok_and(|address| address == state.node_address)
        {
            continue;
        }
        let rank = blake3::hash(format!("replica:{object_id}:{node}").as_bytes())
            .to_hex()
            .to_string();
        ranked.push((rank, node.clone()));
    }
    ranked.sort();
    ranked.reverse();
    ranked.into_iter().map(|(_, node)| node).collect()
}

/// Repairs the set after a flight. A member that failed and is no
/// longer listed is dropped: a node that merely missed a flight is
/// resent from its length next time, but a node that is gone would
/// fail every flight for the rest of the residency. A set short of
/// `replica_count`, then or since (a death with nobody to take its
/// place), is filled with the best-ranked live nodes it lacks, so a
/// node that arrives later joins at the next flight. True when a node
/// joined, so the next flight lays the generation on it.
///
/// Membership is the shared snapshot, refreshed every few seconds and
/// never fetched per flight: a death shows within it, and until then
/// the member fails another flight and is asked about again.
async fn repair(state: &AppState, object_id: &str, set: &ReplicaSet, failed: &[String]) -> bool {
    let short = set
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .len()
        < state.replica_count;
    if failed.is_empty() && !short {
        return false;
    }
    let live = ranked(state, object_id).await;
    let mut members = set.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    for dead in failed {
        if live.iter().any(|node| node == dead) {
            continue;
        }
        if let Some(slot) = members.iter().position(|member| member == dead) {
            actias_common::tracing::info!(object_id, dead, "a replica died; leaving the set");
            members.remove(slot);
        }
    }
    let mut joined = false;
    for node in live {
        if members.len() >= state.replica_count {
            break;
        }
        if members.contains(&node) {
            continue;
        }
        actias_common::tracing::info!(
            object_id,
            replica = %node,
            "the next live node joins the replica set"
        );
        members.push(node);
        joined = true;
    }
    joined
}

/// Builds the fan-out for one residency: every request goes to every
/// replica in parallel. An append behind is resent from the length the
/// replica reports; a generation lay reaches a replica not on the
/// delta's list as every chunk; a fence from any of them marks the
/// flight as lost to a newer owner.
pub fn fanout_for(
    state: AppState,
    object_id: String,
    replicas: ReplicaSet,
    relay: Arc<std::sync::atomic::AtomicBool>,
) -> FanoutFn {
    Arc::new(move |request: FanoutRequest| {
        let state = state.clone();
        let object_id = object_id.clone();
        let set = replicas.clone();
        let relay = relay.clone();
        let replicas = set
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let request = Arc::new(request);
        Box::pin(async move {
            use futures::StreamExt;
            let quorum = request.quorum;
            let mut sends: futures::stream::FuturesUnordered<_> = replicas
                .iter()
                .map(|node| {
                    send_to(
                        state.clone(),
                        node.clone(),
                        object_id.clone(),
                        request.clone(),
                    )
                })
                .collect();
            let mut acks = 0;
            let mut fenced = false;
            let mut failed: Vec<String> = Vec::new();
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
                    Some(Sent::Failed(node)) => failed.push(node),
                    None => break,
                }
                if quorum > 0 && acks >= quorum && grace.is_none() {
                    grace = Some(Box::pin(tokio::time::sleep(STRAGGLER_GRACE)));
                }
            }
            // After the answer, never before it: the repair is
            // bookkeeping for the next flight, not this one's wait. A
            // newcomer has no generation; the next flight rotates so the
            // lay reaches it with everything.
            if repair(&state, &object_id, &set, &failed).await {
                relay.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            FanoutOutcome { acks, fenced }
        })
    })
}

/// Longest exchange with one replica for one flight: the append and at
/// most one correction (a resend from its length), or a delta lay and
/// the full lay a replica off the list asks for.
const ATTEMPTS: usize = 2;

/// How long a flight waits for the replicas past its quorum before it
/// goes on without them.
const STRAGGLER_GRACE: std::time::Duration = std::time::Duration::from_millis(20);

async fn send_to(
    state: AppState,
    node: String,
    object_id: String,
    request: Arc<FanoutRequest>,
) -> Sent {
    let Ok(address) = crate::directory::route::address_of(&state, &node).await else {
        return Sent::Failed(node.clone());
    };
    let Ok(mut client) = peer_client(&state, &address).await else {
        return Sent::Failed(node.clone());
    };
    match &request.payload {
        Payload::Append {
            offset,
            wal,
            covered,
        } => {
            let mut offset = *offset as usize;
            for _ in 0..ATTEMPTS {
                let append = WalAppend {
                    object_id: object_id.clone(),
                    epoch: request.epoch,
                    base: request.base,
                    offset: offset as u64,
                    bytes: wal.slice(offset.min(wal.len())..),
                    covered: *covered,
                };
                let reply = tokio::time::timeout(
                    state.replica_ack,
                    client.append_wal(authed(&state.internal_token, append)),
                )
                .await;
                match settle(&object_id, &node, reply, "append") {
                    Answer::Applied => return Sent::Acked,
                    Answer::Refused(reply) if reply.refusal == refusal::GAP => {
                        offset = reply.length as usize;
                    }
                    Answer::Refused(reply) if reply.refusal == refusal::FENCED => {
                        return Sent::Fenced;
                    }
                    // A replica without the generation waits for the next
                    // lay; this flight goes on without it.
                    Answer::Refused(_) | Answer::Failed => return Sent::Failed(node.clone()),
                }
            }
            Sent::Failed(node.clone())
        }
        Payload::Lay {
            from_list,
            base_len,
            chunks,
            dirty,
            file,
        } => {
            let all: Arc<Vec<u32>> = Arc::new((0..chunks.len() as u32).collect());
            let mut attempt = (from_list.clone(), dirty.clone());
            for _ in 0..ATTEMPTS {
                let (from, indices) = attempt.clone();
                let parts = lay_parts(
                    object_id.clone(),
                    request.epoch,
                    request.base,
                    from,
                    *base_len,
                    chunks.clone(),
                    indices,
                    file.clone(),
                );
                let reply = tokio::time::timeout(
                    state.replica_ack.saturating_mul(LAY_BUDGET_FACTOR),
                    client.lay_generation(authed(&state.internal_token, parts)),
                )
                .await;
                match settle(&object_id, &node, reply, "lay") {
                    Answer::Applied => return Sent::Acked,
                    Answer::Refused(reply) if reply.refusal == refusal::NO_BASE => {
                        // Off the list: everything, then.
                        attempt = (String::new(), all.clone());
                    }
                    Answer::Refused(reply) if reply.refusal == refusal::FENCED => {
                        return Sent::Fenced;
                    }
                    Answer::Refused(_) | Answer::Failed => return Sent::Failed(node.clone()),
                }
            }
            Sent::Failed(node.clone())
        }
    }
}

/// A lay may carry a whole base; it gets this many append budgets.
const LAY_BUDGET_FACTOR: u32 = 30;

enum Answer {
    Applied,
    Refused(WalAppended),
    Failed,
}

fn settle(
    object_id: &str,
    node: &str,
    reply: Result<Result<tonic::Response<WalAppended>, tonic::Status>, tokio::time::error::Elapsed>,
    what: &str,
) -> Answer {
    match reply {
        Ok(Ok(reply)) => {
            let reply = reply.into_inner();
            if reply.applied {
                Answer::Applied
            } else {
                if reply.refusal == refusal::FENCED {
                    actias_common::tracing::warn!(
                        object_id,
                        node,
                        what,
                        "replica fenced this owner"
                    );
                }
                Answer::Refused(reply)
            }
        }
        Ok(Err(status)) => {
            actias_common::tracing::warn!(object_id, node, what, error = %status, "replica call failed");
            Answer::Failed
        }
        Err(_) => {
            actias_common::tracing::warn!(object_id, node, what, "replica call timed out");
            Answer::Failed
        }
    }
}

/// The parts of one generation lay, produced as the stream is polled:
/// the header, then each chunk read from the file when its turn comes,
/// then done. Nothing holds more than one chunk.
#[allow(clippy::too_many_arguments)]
fn lay_parts(
    object_id: String,
    epoch: u64,
    base: u64,
    from_list: String,
    base_len: u64,
    chunks: Arc<Vec<String>>,
    indices: Arc<Vec<u32>>,
    file: PathBuf,
) -> impl futures::Stream<Item = GenerationPart> + Send + 'static {
    use futures::StreamExt;
    let header = GenerationPart {
        part: Some(generation_part::Part::Header(GenerationHeader {
            object_id,
            epoch,
            base,
            from_list,
            base_len,
            chunks: chunks.as_ref().clone(),
        })),
    };
    let body = futures::stream::unfold(0usize, move |at| {
        let indices = indices.clone();
        let file = file.clone();
        async move {
            let index = *indices.get(at)?;
            let part = match crate::objects::store::read_chunk(&file, index).await {
                Ok(bytes) => GenerationPart {
                    part: Some(generation_part::Part::Chunk(GenerationChunk {
                        index,
                        bytes: Bytes::from(bytes),
                    })),
                },
                Err(error) => {
                    actias_common::tracing::warn!(index, %error, "a chunk could not be read for the lay");
                    // The stream ends short of done; the replica refuses
                    // the incomplete lay and drops the generation.
                    return None;
                }
            };
            Some((part, at + 1))
        }
    });
    let done = futures::stream::once(async {
        GenerationPart {
            part: Some(generation_part::Part::Done(GenerationDone {})),
        }
    });
    futures::stream::once(async { header })
        .chain(body)
        .chain(done)
}
