//! The pump: between a publisher's calls, due edges partitioned by node
//! and delivered, locally or in one batch per node.

use super::*;

/// Delivery batch per edge per pump pass; the pump re-arms for the rest.
pub(super) const DELIVERY_BATCH: i64 = 16;

/// Base backoff after a failed delivery, doubling per attempt.
pub(super) const BACKOFF_BASE_MS: i64 = 500;

/// Backoff ceiling.
pub(super) const BACKOFF_CAP_MS: i64 = 60_000;

/// Attempts after which an edge is dropped: bounded patience, the
/// queue's dead-letter discipline applied per edge.
pub(super) const MAX_ATTEMPTS: i64 = 8;

/// One delivery may not hang the pump; a timed-out edge retries later,
/// which also breaks accidental call cycles between publisher and
/// follower.
pub const DELIVERY_TIMEOUT_SECS: u64 = 10;

/// At-most-once delivery to one connection edge: matching events go to
/// the node-local inbox, the watermark advances regardless of outcome
/// (a connection edge never retries, it misses what it misses), and a
/// refusal prunes the edge (Gone or Overflow both mean the connection
/// is not coming back for these events). No registry on this runtime
/// means nothing to deliver to, which is the same prune.
pub(super) fn deliver_connection_edge(
    home: &std::sync::Arc<crate::objects::ObjectHome>,
    edge: &Edge,
    registry: Option<&crate::connections::ConnectionRegistry>,
) {
    let events = match home.with_storage(|storage| events_after(storage, &edge.topic, edge.cursor))
    {
        Ok(events) => events,
        Err(error) => {
            actias_common::tracing::warn!(%error, "stream pump could not read events");
            return;
        }
    };
    let Some(last) = events.last().map(|event| event.seq) else {
        let head = home.with_storage(head_seq).unwrap_or(edge.cursor);
        let _ = home.with_storage(|storage| advance_cursor(storage, edge.id, head));
        return;
    };

    let mut pruned = false;
    if let (Some(registry), Some(connection_id)) = (registry, edge.connection.as_deref()) {
        for event in &events {
            if !filter_matches(edge.filter.as_ref(), &event.data) {
                continue;
            }
            let item = crate::connections::InboxItem::Event {
                topic: event.topic.clone(),
                from_class: event.from_class.clone(),
                from_name: event.from_name.clone(),
                data: event.data.clone(),
            };
            if let Err(refused) = registry.deliver(connection_id, item) {
                actias_common::tracing::debug!(?refused, connection_id, "connection edge pruned");
                pruned = true;
                break;
            }
        }
    } else {
        pruned = true;
    }

    if pruned {
        let _ = home.with_storage(|storage| prune_edge(storage, edge.id));
    } else {
        let _ = home.with_storage(|storage| advance_cursor(storage, edge.id, last));
    }
}

/// One delivery pass over every due object edge: matching events copied
/// to each follower's `receive`, cursors advanced, failures backed off,
/// then the timer re-armed for whatever remains. Runs in the
/// publisher's own task between mailbox items.
pub async fn pump(
    runtime: &crate::runtime::ActiasRuntime,
    home: &std::sync::Arc<crate::objects::ObjectHome>,
) {
    use crate::extensions::objects::{ObjectRouter, ObjectTarget};

    if !home.has_storage() {
        return;
    }
    let Some(router) = runtime
        .app_data_ref::<ObjectRouter>()
        .map(|router| router.clone())
    else {
        actias_common::tracing::warn!("stream delivery has no router; events wait");
        return;
    };

    let now = crate::extensions::objects::unix_now_ms();
    let snapshot = home.with_storage(|storage| {
        let head = head_seq(storage)?;
        let edges = list_edges(storage, None)?;
        Ok((head, edges))
    });
    let (head, edges) = match snapshot {
        Ok(pair) => pair,
        Err(error) => {
            actias_common::tracing::warn!(%error, "stream pump could not read edges");
            return;
        }
    };

    let registry = runtime
        .app_data_ref::<std::sync::Arc<crate::connections::ConnectionRegistry>>()
        .map(|registry| registry.clone());
    let local_node = runtime
        .app_data_ref::<LocalNode>()
        .map(|node| node.0.clone())
        .unwrap_or_default();
    let forwarder = runtime
        .app_data_ref::<ConnectionForwarder>()
        .map(|forwarder| forwarder.clone());
    let receive_forwarder = runtime
        .app_data_ref::<ReceiveForwarder>()
        .map(|forwarder| forwarder.clone());
    let publisher_identity = runtime
        .app_data_ref::<PublisherIdentity>()
        .map(|identity| identity.clone());

    // Remote connection edges batch per node: every node with
    // followers hears one call carrying everything due for it, and
    // fans out to its own sockets. Publish cost is the number of
    // nodes listening, never the number of listeners, and the flush
    // ships each due event once per node however many edges want it.
    let mut remote: std::collections::HashMap<String, Vec<InboxEdge>> =
        std::collections::HashMap::new();
    let mut remote_receives: std::collections::HashMap<String, Vec<ReceiveDelivery>> =
        std::collections::HashMap::new();

    for edge in edges {
        if edge.cursor >= head {
            continue;
        }
        if edge.kind == "connection" {
            let elsewhere = edge
                .node
                .as_deref()
                .filter(|node| !node.is_empty() && !local_node.is_empty() && *node != local_node);
            if let Some(node) = elsewhere {
                stage_remote_edge(&edge, node, &mut remote);
            } else {
                deliver_connection_edge(home, &edge, registry.as_deref());
            }
            continue;
        }
        if edge.next_at > now {
            continue;
        }
        // A durable edge whose follower lives elsewhere batches into
        // that node's one call instead of one routed dispatch per
        // event from here.
        let elsewhere = edge
            .node
            .as_deref()
            .filter(|node| !node.is_empty() && !local_node.is_empty() && *node != local_node);
        if let Some(node) = elsewhere
            && receive_forwarder.is_some()
            && publisher_identity.is_some()
        {
            stage_receive_edge(home, &edge, node, head, &mut remote_receives);
            continue;
        }
        let events =
            match home.with_storage(|storage| events_after(storage, &edge.topic, edge.cursor)) {
                Ok(events) => events,
                Err(error) => {
                    actias_common::tracing::warn!(%error, "stream pump could not read events");
                    continue;
                }
            };
        if events.is_empty() {
            // Other topics advanced the log; nothing here for this edge.
            let _ = home.with_storage(|storage| advance_cursor(storage, edge.id, head));
            continue;
        }

        let mut advanced = edge.cursor;
        let mut failed = false;
        for event in events {
            if !filter_matches(edge.filter.as_ref(), &event.data) {
                advanced = event.seq;
                continue;
            }
            let payload = serde_json::json!({
                "seq": event.seq,
                "topic": event.topic,
                "from": { "class": event.from_class, "name": event.from_name },
                "data": event.data,
            });
            let delivery = router(ObjectTarget {
                class: edge.class.clone(),
                name: edge.name.clone(),
                method: "__receive".to_owned(),
                arguments: vec![payload],
                // A delivery is a fresh causal root, never a nested call:
                // an empty chain keeps publisher/follower ping-pong legal,
                // and the timeout breaks accidental synchronous cycles.
                chain: Vec::new(),
                caller: None,
            });
            match tokio::time::timeout(
                std::time::Duration::from_secs(DELIVERY_TIMEOUT_SECS),
                delivery,
            )
            .await
            {
                Ok(Ok(_)) => advanced = event.seq,
                Ok(Err(error)) => {
                    actias_common::tracing::debug!(%error, "stream delivery refused");
                    failed = true;
                    break;
                }
                Err(_) => {
                    actias_common::tracing::debug!("stream delivery timed out");
                    failed = true;
                    break;
                }
            }
        }

        if advanced > edge.cursor {
            let _ = home.with_storage(|storage| advance_cursor(storage, edge.id, advanced));
        }
        if failed {
            let _ = home.with_storage(|storage| record_failure(storage, edge.id));
        }
    }

    for (node, batch) in remote {
        flush_remote_batch(home, forwarder.as_ref(), &node, batch).await;
    }
    if let (Some(forward), Some(identity)) = (receive_forwarder, publisher_identity) {
        for (node, batch) in remote_receives {
            flush_receive_batch(home, &forward, identity.clone(), &node, batch).await;
        }
    }

    match home.with_storage(next_delivery_due) {
        Ok(due) => home.set_delivery_due(due),
        Err(error) => {
            actias_common::tracing::warn!(%error, "stream pump could not re-arm");
        }
    }
}

/// Records one remote connection edge into its node's batch. No
/// events are read here: the flush reads each due topic once for the
/// whole node, and the receiving node slices per edge, filter
/// included. The cursor does not move here either; the flush owns it.
pub(super) fn stage_remote_edge(
    edge: &Edge,
    node: &str,
    remote: &mut std::collections::HashMap<String, Vec<InboxEdge>>,
) {
    let Some(connection) = edge.connection.clone() else {
        return;
    };
    remote.entry(node.to_owned()).or_default().push(InboxEdge {
        edge_id: edge.id,
        connection,
        topic: edge.topic.clone(),
        after: edge.cursor,
        filter: edge.filter.clone(),
    });
}

/// Reads one durable edge's due, filtered events into its node's
/// batch: inline under the cap, as a range past it (the receiving
/// node reads ranges from the nearest copy of this log).
pub(super) fn stage_receive_edge(
    home: &std::sync::Arc<crate::objects::ObjectHome>,
    edge: &Edge,
    node: &str,
    head: i64,
    remote: &mut std::collections::HashMap<String, Vec<ReceiveDelivery>>,
) {
    let events = match home.with_storage(|storage| events_after(storage, &edge.topic, edge.cursor))
    {
        Ok(events) => events,
        Err(error) => {
            actias_common::tracing::warn!(%error, "stream pump could not read events");
            return;
        }
    };
    let due: Vec<serde_json::Value> = events
        .iter()
        .filter(|event| filter_matches(edge.filter.as_ref(), &event.data))
        .map(|event| {
            serde_json::json!({
                "seq": event.seq,
                "topic": event.topic,
                "from_class": event.from_class,
                "from_name": event.from_name,
                "data": event.data,
            })
        })
        .collect();
    let inline_bytes: usize = due.iter().map(|event| event.to_string().len()).sum();
    let (events, range) = if inline_bytes > INLINE_EVENT_CAP {
        (Vec::new(), Some((edge.cursor, head)))
    } else {
        (due, None)
    };
    remote
        .entry(node.to_owned())
        .or_default()
        .push(ReceiveDelivery {
            edge_id: edge.id,
            follower_class: edge.class.clone(),
            follower_name: edge.name.clone(),
            topic: edge.topic.clone(),
            filter: edge.filter.clone(),
            events,
            range,
        });
}

/// One node's durable batch over the wire, with per-edge outcomes
/// mapped back exactly as per-edge delivery would have scored them:
/// delivered_to advances the cursor, failed backs the edge off, and a
/// transport failure backs off everything it carried (the follower's
/// own cursor makes any redelivery skip instead of re-run).
pub(super) async fn flush_receive_batch(
    home: &std::sync::Arc<crate::objects::ObjectHome>,
    forward: &ReceiveForwarder,
    identity: PublisherIdentity,
    node: &str,
    batch: Vec<ReceiveDelivery>,
) {
    let edge_of: std::collections::HashMap<(String, String), i64> = batch
        .iter()
        .map(|delivery| {
            (
                (
                    delivery.follower_class.clone(),
                    delivery.follower_name.clone(),
                ),
                delivery.edge_id,
            )
        })
        .collect();
    let all_edges: Vec<i64> = batch.iter().map(|delivery| delivery.edge_id).collect();

    match forward(node.to_owned(), identity, batch).await {
        Ok(reports) => {
            for report in reports {
                let key = (report.follower_class, report.follower_name);
                let Some(edge_id) = edge_of.get(&key) else {
                    continue;
                };
                if report.delivered_to > 0 {
                    let _ = home.with_storage(|storage| {
                        advance_cursor(storage, *edge_id, report.delivered_to)
                    });
                }
                if report.failed {
                    let _ = home.with_storage(|storage| record_failure(storage, *edge_id));
                }
            }
        }
        Err(ForwardError::NodeGone) => {
            actias_common::tracing::debug!(
                node,
                "follower's node is gone; edges fall back to routed delivery"
            );
            let _ = home.with_storage(|storage| clear_edge_nodes(storage, &all_edges));
        }
        Err(error @ ForwardError::Transport(_)) => {
            actias_common::tracing::debug!(%error, node, "durable batch missed; backing off");
            for edge_id in all_edges {
                let _ = home.with_storage(|storage| record_failure(storage, edge_id));
            }
        }
    }
}

/// One node's batch over the wire: the due events once (one log read
/// per topic at that topic's furthest-behind edge, sorted back into
/// publish order by seq), and every edge they are due for. At-most-once
/// all the way: cursors advance to head whatever happens (a miss is a
/// miss), and only an explicit "gone" from the hosting node prunes,
/// which drops every edge that connection held. A transport failure
/// keeps the edges for next time.
pub(super) async fn flush_remote_batch(
    home: &std::sync::Arc<crate::objects::ObjectHome>,
    forwarder: Option<&ConnectionForwarder>,
    node: &str,
    edges: Vec<InboxEdge>,
) {
    let head = home.with_storage(head_seq).unwrap_or(0);
    let mut edges_of: std::collections::HashMap<String, Vec<i64>> =
        std::collections::HashMap::new();
    let mut after_of: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for edge in &edges {
        edges_of
            .entry(edge.connection.clone())
            .or_default()
            .push(edge.edge_id);
        after_of
            .entry(edge.topic.clone())
            .and_modify(|after| *after = (*after).min(edge.after))
            .or_insert(edge.after);
    }
    let edge_ids: Vec<i64> = edges.iter().map(|edge| edge.edge_id).collect();

    let mut events: Vec<serde_json::Value> = Vec::new();
    for (topic, after) in after_of {
        let read = home.with_storage(|storage| events_after(storage, &topic, after));
        match read {
            Ok(list) => events.extend(list.iter().map(|event| {
                serde_json::json!({
                    "seq": event.seq,
                    "topic": event.topic,
                    "from_class": event.from_class,
                    "from_name": event.from_name,
                    "data": event.data,
                })
            })),
            Err(error) => {
                actias_common::tracing::warn!(%error, "stream pump could not read events");
            }
        }
    }
    events.sort_by_key(|event| event["seq"].as_i64().unwrap_or(0));

    let gone = match forwarder {
        Some(forward) => match forward(node.to_owned(), NodeInbox { events, edges }).await {
            Ok(gone) => gone,
            Err(error) => {
                actias_common::tracing::debug!(%error, node, "remote inbox batch missed");
                Vec::new()
            }
        },
        None => {
            actias_common::tracing::debug!(node, "no forwarder; remote connection events missed");
            Vec::new()
        }
    };

    for edge_id in edge_ids {
        let _ = home.with_storage(|storage| advance_cursor(storage, edge_id, head));
    }
    for connection in gone {
        for edge_id in edges_of.remove(&connection).unwrap_or_default() {
            let _ = home.with_storage(|storage| prune_edge(storage, edge_id));
        }
    }
}
