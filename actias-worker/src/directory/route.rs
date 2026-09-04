//! Reader placement: which node answers a class's directory queries.
//!
//! Without it, any node asked about any class materializes that whole
//! class locally (base plus deltas into an overlay file), so a class
//! queried from N nodes is built N times and rebuilt N times per
//! generation, and the read side's cost grows with cluster width.
//!
//! Each class has a reader: the live node that wins a rendezvous hash
//! over the class key, the same choice reconciliation makes for its
//! owner, so the two agree without talking. A query arriving anywhere
//! else takes one hop to the reader over the data plane and comes back
//! as the same page. The reader is a preference, never a dependency: a
//! hop that fails for any reason answers locally, so a partition costs
//! one extra overlay rather than an error. A hop carries a marker so
//! the reader serves rather than forwards again; two hops can never
//! happen.
//!
//! Reads never coordinate: no lease, no registry write, nothing woken.
//! The reader is derived from membership, which the heartbeat loop
//! already maintains, and cached for a few seconds so a query costs no
//! registry read on the hot path.
use std::sync::Arc;

use actias_worker_core::directory::overlay::{Entry, Page, Query};
use actias_worker_core::proto::worker_data::{
    DirectoryCondition, DirectoryOrder, DirectoryQuery, DirectoryWhere,
};

use crate::directory::sync::ClassKey;
use crate::server::AppState;
use actias_worker_core::directory::verify::{Visited, VisitedPage};

/// The metadata key a forwarded query carries, so the reader knows to
/// answer rather than route.
pub const HOP_KEY: &str = "x-actias-directory-hop";

// The membership snapshot's ttl lives on the cache itself
// (`AppState::reader_membership`, ten seconds): a node that joins or
// leaves is seen within it, and until then a query may hop to a node
// that is gone, which the fallback absorbs. Replica ranking reads the
// same snapshot, so a flight never costs a registry read.

/// The reader for one class among the nodes alive as of the last
/// membership read; [`None`] when it is this node (or nobody is
/// known), which means answer locally.
async fn reader_for(state: &AppState, class: &ClassKey) -> Option<String> {
    let own = state.node_identity.read().ok()?.clone()?;
    let nodes = live_nodes_cached(state).await;
    if nodes.len() <= 1 {
        return None;
    }
    let winner = nodes.iter().max_by_key(|node| {
        blake3::hash(format!("reader:{}:{}:{}", class.scope_id, class.class, node).as_bytes())
            .to_hex()
            .to_string()
    })?;
    (*winner != own).then(|| winner.clone())
}

pub(crate) async fn live_nodes_cached(state: &AppState) -> Arc<Vec<String>> {
    if let Some(nodes) = state.reader_membership.get("nodes").await {
        return nodes;
    }
    let nodes = Arc::new(crate::directory::rebuild::live_nodes(state).await);
    state
        .reader_membership
        .insert("nodes".to_owned(), nodes.clone())
        .await;
    nodes
}

/// Resolves a node's data-plane address the way object forwarding
/// does: the cache, then the registry, remembered.
pub(crate) async fn address_of(state: &AppState, node_id: &str) -> Result<String, String> {
    if let Some(address) = state.node_addrs.get(node_id).await {
        return Ok(address);
    }
    let node = state
        .registry
        .clone()
        .get_node(actias_worker_core::proto::node_registry::GetNodeRequest {
            node_id: node_id.to_owned(),
        })
        .await
        .map_err(|error| error.message().to_owned())?
        .into_inner();
    state
        .node_addrs
        .insert(node_id.to_owned(), node.address.clone())
        .await;
    Ok(node.address)
}

/// Whether a request arrived by a hop, read before the message is
/// taken out of it.
pub fn is_hop<T>(request: &tonic::Request<T>) -> bool {
    request.metadata().get(HOP_KEY).is_some()
}

pub(crate) fn hopped<T>(state: &AppState, message: T) -> tonic::Request<T> {
    let mut request = crate::data_plane::authed(&state.internal_token, message);
    if let Ok(value) = "1".parse() {
        request.metadata_mut().insert(HOP_KEY, value);
    }
    request
}

/// The wire form of a kernel query, for the hop. The inverse of
/// `crate::directory::query::where_from_proto`, and a translation rather than
/// a copy: the kernel's tree is what both sides evaluate.
fn to_wire(class: &ClassKey, query: &Query) -> DirectoryQuery {
    DirectoryQuery {
        scope_id: class.scope_id.clone(),
        class: class.class.clone(),
        r#where: Some(where_to_proto(&query.where_)),
        order: query
            .order
            .iter()
            .map(|order| DirectoryOrder {
                field: order.field.clone(),
                descending: order.descending,
            })
            .collect(),
        limit: query.limit,
        cursor: query.cursor.clone(),
    }
}

fn where_to_proto(where_: &actias_worker_core::directory::predicate::Where) -> DirectoryWhere {
    use actias_worker_core::directory::predicate::{Compare, Condition};
    let mut wire = DirectoryWhere::default();
    let condition = |field: &str, op: &str, value: String| DirectoryCondition {
        field: field.to_owned(),
        op: op.to_owned(),
        value_json: value,
    };
    let json = crate::directory::query::value_to_json;
    for entry in &where_.0 {
        match entry {
            Condition::Compare { field, op, value } => {
                let op = match op {
                    Compare::Eq => "eq",
                    Compare::Ne => "ne",
                    Compare::Lt => "lt",
                    Compare::Lte => "lte",
                    Compare::Gt => "gt",
                    Compare::Gte => "gte",
                };
                wire.conditions.push(condition(field, op, json(value)));
            }
            Condition::In { field, values } => {
                let list = actias_worker_core::directory::shape::Value::Array(values.clone());
                wire.conditions
                    .push(condition(field, "one_of", json(&list)));
            }
            Condition::StartsWith { field, prefix } => {
                let text = actias_worker_core::directory::shape::Value::Text(prefix.clone());
                wire.conditions
                    .push(condition(field, "starts_with", json(&text)));
            }
            Condition::Contains { field, value } => {
                wire.conditions
                    .push(condition(field, "contains", json(value)));
            }
            Condition::Exists { field, present } => {
                let flag = actias_worker_core::directory::shape::Value::Bool(*present);
                wire.conditions
                    .push(condition(field, "exists", json(&flag)));
            }
            Condition::Any(branches) => wire.any.extend(branches.iter().map(where_to_proto)),
            Condition::All(branches) => wire.all.extend(branches.iter().map(where_to_proto)),
            Condition::None(branches) => wire.none.extend(branches.iter().map(where_to_proto)),
        }
    }
    wire
}

fn entry_from_wire(
    wire: actias_worker_core::proto::worker_data::DirectoryEntry,
) -> Result<Entry, String> {
    let mut fields = Vec::with_capacity(wire.fields.len());
    for (name, raw) in wire.fields {
        let value = crate::directory::query::value_from_json(&raw, &name)?;
        fields.push((name, value));
    }
    fields.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(Entry {
        name: wire.name,
        object_id: wire.object_id,
        fields,
    })
}

/// A listing with the class's building fields, answered by the class's
/// reader when that is another node and the hop succeeds, locally
/// otherwise. The building list rides the hop too, so a forwarded page
/// costs this node no overlay at all. A refusal the reader
/// returned (an unknown field, a building one) is the caller's answer
/// and comes back as such; only transport falls back.
pub async fn list(
    state: &AppState,
    class: &ClassKey,
    query: Query,
) -> Result<(Page, Vec<String>), String> {
    let _query = state
        .shares
        .directory_queries
        .acquire(&class.scope_id)
        .await;
    if let Some(reader) = reader_for(state, class).await {
        match hop_list(state, &reader, class, &query).await {
            Ok(page) => {
                state
                    .directory_gauges
                    .count(&state.directory_gauges.forwarded);
                return Ok(page);
            }
            Err(Hop::Refused(message)) => return Err(message),
            Err(Hop::Transport(error)) => actias_common::tracing::debug!(
                class = %class.class,
                %reader,
                %error,
                "the class's reader did not answer; answering locally"
            ),
        }
    }
    let page = crate::directory::read::list(state, class, query).await?;
    let building = crate::directory::read::building(state, class).await;
    Ok((page, building))
}

/// A verified read, routed the same way.
pub async fn visit(
    state: &AppState,
    class: &ClassKey,
    query: Query,
) -> Result<(VisitedPage, Vec<String>), String> {
    let _query = state
        .shares
        .directory_queries
        .acquire(&class.scope_id)
        .await;
    if let Some(reader) = reader_for(state, class).await {
        match hop_visit(state, &reader, class, &query).await {
            Ok(page) => {
                state
                    .directory_gauges
                    .count(&state.directory_gauges.forwarded);
                return Ok(page);
            }
            Err(Hop::Refused(message)) => return Err(message),
            Err(Hop::Transport(error)) => actias_common::tracing::debug!(
                class = %class.class,
                %reader,
                %error,
                "the class's reader did not answer; visiting locally"
            ),
        }
    }
    let page = crate::directory::visit::visit(state, class, query).await?;
    let building = crate::directory::read::building(state, class).await;
    Ok((page, building))
}

/// A refusal the reader returned is the caller's answer (an unknown
/// field, a building one) and must come back as such, not as a failed
/// hop that then answers locally with the same refusal after a second
/// overlay build. Anything else is transport, which falls back.
enum Hop {
    Refused(String),
    Transport(String),
}

async fn hop_list(
    state: &AppState,
    reader: &str,
    class: &ClassKey,
    query: &Query,
) -> Result<(Page, Vec<String>), Hop> {
    let address = address_of(state, reader).await.map_err(Hop::Transport)?;
    let mut client = crate::data_plane::peer_client(state, &address)
        .await
        .map_err(Hop::Transport)?;
    let answer = client
        .list_directory(hopped(state, to_wire(class, query)))
        .await
        .map_err(|status| match status.code() {
            tonic::Code::InvalidArgument => Hop::Refused(status.message().to_owned()),
            _ => Hop::Transport(status.to_string()),
        })?
        .into_inner();
    let entries = answer
        .entries
        .into_iter()
        .map(entry_from_wire)
        .collect::<Result<Vec<_>, _>>()
        .map_err(Hop::Transport)?;
    Ok((
        Page {
            entries,
            cursor: answer.cursor,
        },
        answer.building,
    ))
}

async fn hop_visit(
    state: &AppState,
    reader: &str,
    class: &ClassKey,
    query: &Query,
) -> Result<(VisitedPage, Vec<String>), Hop> {
    let address = address_of(state, reader).await.map_err(Hop::Transport)?;
    let mut client = crate::data_plane::peer_client(state, &address)
        .await
        .map_err(Hop::Transport)?;
    let answer = client
        .visit_directory(hopped(state, to_wire(class, query)))
        .await
        .map_err(|status| match status.code() {
            tonic::Code::InvalidArgument => Hop::Refused(status.message().to_owned()),
            _ => Hop::Transport(status.to_string()),
        })?
        .into_inner();
    let mut entries = Vec::with_capacity(answer.entries.len());
    for served in answer.entries {
        let Some(entry) = served.entry else { continue };
        entries.push(Visited {
            entry: entry_from_wire(entry).map_err(Hop::Transport)?,
            unverified: served.unverified,
            reason: (!served.reason.is_empty()).then_some(served.reason),
        });
    }
    Ok((
        VisitedPage {
            entries,
            cursor: answer.cursor,
        },
        answer.building,
    ))
}
