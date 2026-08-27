//! The worker's data plane: the WorkerData grpc service every node serves
//! and every node (plus the api) calls. Dispatch runs an object method
//! here; the reads answer from the freshest copy this node can reach:
//! the local file, else the lease holder's node, else the shipped
//! snapshot replica, else nothing. Transport decoding only; placement and
//! code resolution live in [`crate::routing`], file-to-value in
//! worker-core's platform module.

use tonic::transport::Channel;
use tonic::{Request, Response, Status};

use actias_worker_core::extensions::objects::{CallerIdentity, ObjectTarget};
use actias_worker_core::identity::ObjectKey;
use actias_worker_core::platform::PlatformRead;
use actias_worker_core::proto::node_registry::{GetLeaseRequest, GetNodeRequest};
use actias_worker_core::proto::worker_data::worker_data_client::WorkerDataClient;
use actias_worker_core::proto::worker_data::worker_data_server::WorkerData;
use actias_worker_core::proto::worker_data::{
    CallResult, ConnectionEvents, InboxBatch, InboxReceipts, ObjectCall, ReadRequest, ReadValue,
    ReceiveBatch, ReceiveEntry, ReceiveOutcome, ReceiveReceipts,
};

use crate::routing::{ObjectRouting, fresh_replica_file, owner_prepared};
use crate::server::AppState;

/// The metadata key carrying the cluster-internal secret.
pub const INTERNAL_TOKEN_KEY: &str = "x-actias-internal";

/// The interceptor gating every data-plane rpc: the shared secret in
/// metadata, or nothing.
// A Status-sized Err is tonic's interceptor contract, not a choice.
#[allow(clippy::result_large_err)]
pub fn require_internal_token(
    token: String,
) -> impl Fn(Request<()>) -> Result<Request<()>, Status> + Clone {
    move |request: Request<()>| {
        let authorized = request
            .metadata()
            .get(INTERNAL_TOKEN_KEY)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value == token);
        if authorized {
            Ok(request)
        } else {
            Err(Status::unauthenticated("Internal transport only."))
        }
    }
}

/// Wraps a message with the internal token attached, for calls to peers.
pub(crate) fn authed<T>(token: &str, message: T) -> Request<T> {
    let mut request = Request::new(message);
    // The token is operator-configured ascii; one that cannot be metadata
    // simply travels absent and the peer refuses the call.
    if let Ok(value) = token.parse() {
        request.metadata_mut().insert(INTERNAL_TOKEN_KEY, value);
    }
    request
}

/// A client for one peer's data plane, over a cached lazy channel:
/// a dead peer costs its caller the failure, never a held-up cache.
pub(crate) async fn peer_client(
    state: &AppState,
    address: &str,
) -> Result<WorkerDataClient<actias_worker_core::Grpc>, String> {
    if let Some(channel) = state.peers.get(address).await {
        return Ok(WorkerDataClient::new(actias_worker_core::plain_grpc(
            channel,
        )));
    }
    let endpoint = Channel::from_shared(format!("http://{address}"))
        .map_err(|_| "The peer's address is not routable.".to_owned())?;
    let channel = endpoint.connect_lazy();
    state
        .peers
        .insert(address.to_owned(), channel.clone())
        .await;
    Ok(WorkerDataClient::new(actias_worker_core::plain_grpc(
        channel,
    )))
}

/// The read a request asks for; `sql` outranks `messages` outranks the
/// class's default overview.
// A Status-sized Err is the rpc surface's contract, not a choice.
#[allow(clippy::result_large_err)]
fn stats_read(request: &ReadRequest) -> Result<PlatformRead, Status> {
    match request.sql.clone() {
        Some(sql) => Ok(PlatformRead::Query { sql }),
        None if request.messages => Ok(PlatformRead::QueueMessages),
        None if request.followers => Ok(PlatformRead::Followers),
        None => PlatformRead::stats_for_class(&request.class)
            .ok_or_else(|| Status::invalid_argument("No stats for that class.")),
    }
}

pub struct WorkerDataService {
    state: AppState,
}

impl WorkerDataService {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }

    /// One read against the freshest copy reachable from this node.
    async fn read_routed(
        &self,
        request: ReadRequest,
        read: PlatformRead,
    ) -> Result<Response<ReadValue>, Status> {
        let key = ObjectKey::received(&request.scope_id, &request.class, &request.name);
        let local = self.state.object_data_dir.join(key.db_file_name());

        // The holder's live file is the freshest copy there is; when it is
        // not ours, a first hop asks the holder's node before settling for
        // the shipped replica.
        let file = if local.exists() {
            Some(local)
        } else {
            if request.first_hop
                && let Some(value) = self.read_from_holder(&key, &request).await?
            {
                return Ok(Response::new(value));
            }
            fresh_replica_file(&self.state, &key.object_id())
                .await
                .map_err(Status::invalid_argument)?
        };
        let Some(file) = file else {
            // Nothing local and nothing ever shipped: the object has no
            // observable state yet, which reads as empty, not an error.
            return Ok(Response::new(ReadValue {
                value_json: serde_json::Value::Null.to_string(),
            }));
        };

        let value = tokio::task::spawn_blocking(move || read.run(&file))
            .await
            .map_err(|error| {
                actias_common::tracing::error!(%error, "platform read task died");
                Status::internal("The read failed.")
            })?
            .map_err(Status::invalid_argument)?;

        Ok(Response::new(ReadValue {
            value_json: value.to_string(),
        }))
    }

    /// The holder's answer for a read this node cannot serve locally.
    /// [`Ok(None)`] falls through to the replica: nobody holds it, we
    /// hold it (with no file yet), or the holder's node is unreachable.
    /// A holder that answered a refusal is the answer; the same read
    /// would refuse everywhere.
    async fn read_from_holder(
        &self,
        key: &ObjectKey,
        request: &ReadRequest,
    ) -> Result<Option<ReadValue>, Status> {
        let Ok(lease) = self
            .state
            .registry
            .clone()
            .get_lease(GetLeaseRequest {
                object_id: key.object_id(),
            })
            .await
        else {
            return Ok(None);
        };
        let lease = lease.into_inner();

        // Holding the lease without the file only happens mid-spawn;
        // asking ourselves would loop, the replica answers instead.
        let own = self
            .state
            .node_identity
            .read()
            .expect("no poisoned lock")
            .clone();
        if own.as_deref() == Some(lease.node_id.as_str()) {
            return Ok(None);
        }

        let Ok(node) = self
            .state
            .registry
            .clone()
            .get_node(GetNodeRequest {
                node_id: lease.node_id,
            })
            .await
        else {
            return Ok(None);
        };
        let Ok(mut client) = peer_client(&self.state, &node.into_inner().address).await else {
            return Ok(None);
        };

        let forwarded = ReadRequest {
            first_hop: false,
            ..request.clone()
        };
        match client
            .read_stats(authed(&self.state.internal_token, forwarded))
            .await
        {
            Ok(value) => Ok(Some(value.into_inner())),
            // The holder understood and refused (bad sql, no stats for
            // the class): that verdict would be the same anywhere.
            Err(status) if status.code() == tonic::Code::InvalidArgument => Err(status),
            // The holder itself is unreachable or broken; the replica is
            // the graceful degradation.
            Err(_) => Ok(None),
        }
    }
}

#[tonic::async_trait]
impl WorkerData for WorkerDataService {
    /// One object method call: resolve the owner's current code, route to
    /// the pinned vm (forwarding once to the lease holder when this is a
    /// first hop). Method failures ride the envelope; they are the
    /// object's own user-safe errors.
    async fn dispatch(&self, request: Request<ObjectCall>) -> Result<Response<CallResult>, Status> {
        let call = request.into_inner();

        let answer = async {
            let arguments: Vec<serde_json::Value> = if call.arguments_json.is_empty() {
                Vec::new()
            } else {
                serde_json::from_str(&call.arguments_json)
                    .map_err(|e| format!("Malformed object call arguments: {e}"))?
            };

            // The call names an identity, not code: the owner's current
            // revision is resolved here, exactly as a local touch would.
            let key = ObjectKey::received(&call.scope_id, &call.class, &call.name);
            let owner = owner_prepared(&self.state, &key).await?;

            ObjectRouting::new(&self.state, owner)
                .route_inner(
                    ObjectTarget {
                        class: call.class,
                        name: call.name,
                        method: call.method,
                        arguments,
                        // A forwarding sender already extended the chain
                        // through the target; dispatch reads it as-is.
                        chain: call.chain,
                        // The wire's caller is the truth; the owner
                        // resolved here is who RUNS the code, not who
                        // called.
                        caller: call.caller.map(|caller| CallerIdentity {
                            script: caller.script,
                            revision: caller.revision,
                        }),
                    },
                    call.first_hop,
                )
                .await
        }
        .await;

        Ok(Response::new(match answer {
            Ok(result) => CallResult {
                result_json: result.to_string(),
                error: String::new(),
                wrong_home: false,
            },
            // The typed refusal crosses the wire as its own flag, so the
            // sender re-resolves instead of parsing message text.
            Err(crate::routing::RouteError::WrongHome { holder }) => CallResult {
                result_json: String::new(),
                error: format!("Object is homed on {holder}; this node cannot serve it."),
                wrong_home: true,
            },
            Err(crate::routing::RouteError::Failed(error)) => CallResult {
                result_json: String::new(),
                error,
                wrong_home: false,
            },
        }))
    }

    async fn read_stats(
        &self,
        request: Request<ReadRequest>,
    ) -> Result<Response<ReadValue>, Status> {
        let request = request.into_inner();
        let read = stats_read(&request)?;
        self.read_routed(request, read).await
    }

    async fn read_journal(
        &self,
        request: Request<ReadRequest>,
    ) -> Result<Response<ReadValue>, Status> {
        let request = request.into_inner();
        // The journal a class keeps is its own: queues have the delivery
        // journal, workflows the replay journal.
        let read = if request.class == actias_common::classes::WORKFLOW_CLASS {
            PlatformRead::WorkflowJournal {
                since: request.since,
            }
        } else {
            PlatformRead::QueueEvents {
                since: request.since,
            }
        };
        self.read_routed(request, read).await
    }

    /// One publisher's due events for the connections THIS node hosts:
    /// walk the local registry, report back whoever is gone. This is
    /// the receiving half of node-grouped fan-out; the publisher sent
    /// one call for everything here instead of one per socket.
    async fn deliver_inbox(
        &self,
        request: Request<InboxBatch>,
    ) -> Result<Response<InboxReceipts>, Status> {
        let batch = request.into_inner();
        let mut gone = Vec::new();
        for entry in batch.entries {
            let Ok(events) = serde_json::from_str::<Vec<serde_json::Value>>(&entry.events_json)
            else {
                continue;
            };
            for event in events {
                let item = actias_worker_core::connections::InboxItem::Event {
                    topic: event["topic"].as_str().unwrap_or_default().to_owned(),
                    from_class: event["from_class"].as_str().unwrap_or_default().to_owned(),
                    from_name: event["from_name"].as_str().unwrap_or_default().to_owned(),
                    data: event["data"].clone(),
                };
                if self
                    .state
                    .connections
                    .deliver(&entry.connection, item)
                    .is_err()
                {
                    gone.push(entry.connection.clone());
                    break;
                }
            }
        }
        Ok(Response::new(InboxReceipts { gone }))
    }

    /// One publisher's due events for the durable followers THIS node
    /// hosts: materialize each entry's events (inline, or a range read
    /// from the nearest copy of the publisher's log), dispatch
    /// __receive in order, report how far each follower got. The
    /// follower's own cursor makes redelivery skip, so a repeat of
    /// anything here is safe.
    async fn deliver_receives(
        &self,
        request: Request<ReceiveBatch>,
    ) -> Result<Response<ReceiveReceipts>, Status> {
        let batch = request.into_inner();
        let mut outcomes = Vec::new();
        for entry in batch.entries {
            let outcome = self
                .deliver_one_receive(
                    &batch.scope_id,
                    &batch.publisher_class,
                    &batch.publisher_name,
                    entry,
                )
                .await;
            outcomes.push(outcome);
        }
        Ok(Response::new(ReceiveReceipts { outcomes }))
    }
}

impl WorkerDataService {
    async fn deliver_one_receive(
        &self,
        scope: &str,
        publisher_class: &str,
        publisher_name: &str,
        entry: ReceiveEntry,
    ) -> ReceiveOutcome {
        let failed = |delivered_to: i64| ReceiveOutcome {
            follower_class: entry.follower_class.clone(),
            follower_name: entry.follower_name.clone(),
            delivered_to,
            failed: true,
        };

        // The events, from the wire or from the nearest copy of the
        // publisher's log (read_routed prefers the local file, then
        // the holder, then the shipped replica).
        let events: Vec<serde_json::Value> = if !entry.events_json.is_empty() {
            match serde_json::from_str(&entry.events_json) {
                Ok(events) => events,
                Err(_) => return failed(0),
            }
        } else {
            let read = PlatformRead::StreamEvents {
                topic: entry.topic.clone(),
                after: entry.range_after,
                upto: entry.range_upto,
            };
            let request = ReadRequest {
                scope_id: scope.to_owned(),
                class: publisher_class.to_owned(),
                name: publisher_name.to_owned(),
                sql: None,
                messages: false,
                followers: false,
                since: 0,
                first_hop: true,
            };
            let value = match self.read_routed(request, read).await {
                Ok(response) => response.into_inner().value_json,
                Err(_) => return failed(0),
            };
            let filter: Option<serde_json::Value> = if entry.filter_json.is_empty() {
                None
            } else {
                serde_json::from_str(&entry.filter_json).ok()
            };
            match serde_json::from_str::<Vec<serde_json::Value>>(&value) {
                Ok(events) => events
                    .into_iter()
                    .filter(|event| {
                        actias_worker_core::streams::filter_matches(filter.as_ref(), &event["data"])
                    })
                    .collect(),
                Err(_) => return failed(0),
            }
        };

        let follower_key = ObjectKey::received(scope, &entry.follower_class, &entry.follower_name);
        let mut delivered_to = 0;
        for event in events {
            let payload = serde_json::json!({
                "seq": event["seq"],
                "topic": event["topic"],
                "from": { "class": event["from_class"], "name": event["from_name"] },
                "data": event["data"],
            });
            let answer = async {
                let owner = owner_prepared(&self.state, &follower_key).await?;
                ObjectRouting::new(&self.state, owner)
                    .route_inner(
                        ObjectTarget {
                            class: entry.follower_class.clone(),
                            name: entry.follower_name.clone(),
                            method: "__receive".to_owned(),
                            arguments: vec![payload],
                            // A delivery is a fresh causal root, exactly
                            // as the publisher-side pump treats it.
                            chain: Vec::new(),
                            caller: None,
                        },
                        // One forward is allowed: the home hint that
                        // brought the batch here may be stale.
                        true,
                    )
                    .await
            }
            .await;
            match answer {
                Ok(_) => delivered_to = event["seq"].as_i64().unwrap_or(delivered_to),
                Err(error) => {
                    actias_common::tracing::debug!(%error, "batched receive refused");
                    return failed(delivered_to);
                }
            }
        }
        ReceiveOutcome {
            follower_class: entry.follower_class,
            follower_name: entry.follower_name,
            delivered_to,
            failed: false,
        }
    }
}

/// The publisher's half of node-grouped fan-out: resolve the node,
/// send its batch in one call, hand back who it reported gone. Handed
/// to worker-core as the stream pump's [`ConnectionForwarder`].
pub(crate) fn connection_forwarder(
    state: &crate::server::AppState,
) -> actias_worker_core::streams::ConnectionForwarder {
    let state = state.clone();
    std::sync::Arc::new(
        move |node: String, deliveries: Vec<actias_worker_core::streams::RemoteDelivery>| {
            let state = state.clone();
            Box::pin(async move {
                let resolved = match state
                    .registry
                    .clone()
                    .get_node(GetNodeRequest { node_id: node })
                    .await
                {
                    Ok(node) => node.into_inner(),
                    // A node id never returns (a restarted worker registers
                    // a fresh one), so every socket that lived there died
                    // with it: prune the lot.
                    Err(status) if status.code() == tonic::Code::NotFound => {
                        return Ok(deliveries
                            .into_iter()
                            .map(|delivery| delivery.connection)
                            .collect());
                    }
                    Err(e) => {
                        return Err(format!("the follower's node could not be resolved: {e}"));
                    }
                };
                let mut client = peer_client(&state, &resolved.address).await?;
                let entries = deliveries
                    .into_iter()
                    .map(|delivery| ConnectionEvents {
                        connection: delivery.connection,
                        events_json: serde_json::Value::Array(delivery.events).to_string(),
                    })
                    .collect();
                let receipts = client
                    .deliver_inbox(authed(&state.internal_token, InboxBatch { entries }))
                    .await
                    .map_err(|e| e.message().to_owned())?
                    .into_inner();
                Ok(receipts.gone)
            })
        },
    )
}

/// The publisher's half of durable node-grouped fan-out; handed to
/// worker-core as the pump's [`ReceiveForwarder`].
pub(crate) fn receive_forwarder(
    state: &crate::server::AppState,
) -> actias_worker_core::streams::ReceiveForwarder {
    let state = state.clone();
    std::sync::Arc::new(move |node: String, identity, deliveries| {
        let state = state.clone();
        Box::pin(async move {
            use actias_worker_core::streams::ForwardError;
            let resolved = match state
                .registry
                .clone()
                .get_node(GetNodeRequest { node_id: node })
                .await
            {
                Ok(node) => node.into_inner(),
                // The follower rehomed when its node died; clearing the
                // stale home upstream makes delivery route by identity.
                Err(status) if status.code() == tonic::Code::NotFound => {
                    return Err(ForwardError::NodeGone);
                }
                Err(e) => {
                    return Err(ForwardError::Transport(format!(
                        "the follower's node could not be resolved: {e}"
                    )));
                }
            };
            let mut client = peer_client(&state, &resolved.address)
                .await
                .map_err(ForwardError::Transport)?;
            let entries = deliveries
                .into_iter()
                .map(|delivery| ReceiveEntry {
                    follower_class: delivery.follower_class,
                    follower_name: delivery.follower_name,
                    events_json: if delivery.events.is_empty() {
                        String::new()
                    } else {
                        serde_json::Value::Array(delivery.events).to_string()
                    },
                    range_after: delivery.range.map(|range| range.0).unwrap_or_default(),
                    range_upto: delivery.range.map(|range| range.1).unwrap_or_default(),
                    topic: delivery.topic,
                    filter_json: delivery
                        .filter
                        .map(|filter| filter.to_string())
                        .unwrap_or_default(),
                })
                .collect();
            let receipts = client
                .deliver_receives(authed(
                    &state.internal_token,
                    ReceiveBatch {
                        scope_id: identity.scope,
                        publisher_class: identity.class,
                        publisher_name: identity.name,
                        entries,
                    },
                ))
                .await
                .map_err(|e| ForwardError::Transport(e.message().to_owned()))?
                .into_inner();
            Ok(receipts
                .outcomes
                .into_iter()
                .map(|outcome| actias_worker_core::streams::ReceiveReport {
                    follower_class: outcome.follower_class,
                    follower_name: outcome.follower_name,
                    delivered_to: outcome.delivered_to,
                    failed: outcome.failed,
                })
                .collect())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::test_state::{empty_caches, state_with};
    use actias_worker_core::proto::worker_data::worker_data_server::WorkerDataServer;

    #[test]
    fn the_interceptor_admits_only_the_shared_secret() {
        let gate = require_internal_token("right-token".to_owned());

        let refused = gate(Request::new(()));
        assert!(refused.is_err(), "an unauthenticated call must be refused");

        let admitted = gate(authed("right-token", ()));
        assert!(admitted.is_ok());

        let wrong = gate(authed("wrong-token", ()));
        assert!(
            wrong.is_err_and(|status| status.code() == tonic::Code::Unauthenticated),
            "a wrong token must be refused"
        );
    }

    #[test]
    fn the_read_selector_ranks_sql_over_messages_over_class() {
        let request = |sql: Option<&str>, messages: bool, class: &str| ReadRequest {
            scope_id: "p".into(),
            class: class.into(),
            name: "n".into(),
            sql: sql.map(str::to_owned),
            messages,
            since: 0,
            first_hop: false,
            followers: false,
        };

        assert!(matches!(
            stats_read(&request(Some("SELECT 1"), true, "__queue")),
            Ok(PlatformRead::Query { .. })
        ));
        assert!(matches!(
            stats_read(&request(None, true, "__queue")),
            Ok(PlatformRead::QueueMessages)
        ));
        assert!(matches!(
            stats_read(&request(None, false, "__queue")),
            Ok(PlatformRead::QueueStats)
        ));
        assert!(matches!(
            stats_read(&request(None, false, "Warehouse")),
            Ok(PlatformRead::DatabaseOverview)
        ));
        // Platform classes without an overview refuse instead of guessing.
        assert!(
            stats_read(&request(None, false, "__cron"))
                .is_err_and(|status| status.code() == tonic::Code::InvalidArgument)
        );
    }

    /// The served surface end to end: a wrong token never reaches a
    /// handler, the right one does (and fails in the handler, because the
    /// test state's backends are unreachable, proving the gate is the
    /// interceptor, not the handler).
    #[tokio::test(flavor = "multi_thread")]
    async fn the_data_plane_refuses_strangers_and_serves_the_cluster() {
        let state = state_with(empty_caches());
        let token = state.internal_token.clone();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(
            tonic::transport::Server::builder()
                .add_service(WorkerDataServer::with_interceptor(
                    WorkerDataService::new(state),
                    require_internal_token(token.clone()),
                ))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener)),
        );

        let channel = Channel::from_shared(format!("http://{address}"))
            .unwrap()
            .connect()
            .await
            .unwrap();
        let mut client = WorkerDataClient::new(channel);

        let call = || ObjectCall {
            scope_id: "0b7f9ad2-0000-0000-0000-000000000000".to_owned(),
            class: "__queue".to_owned(),
            name: "jobs".to_owned(),
            method: "stats".to_owned(),
            arguments_json: "[]".to_owned(),
            chain: vec![],
            caller: None,
            first_hop: true,
        };

        let refused = client.dispatch(authed("wrong-token", call())).await;
        assert!(
            refused.is_err_and(|status| status.code() == tonic::Code::Unauthenticated),
            "a stranger must be refused"
        );

        // The right token reaches the handler, whose owner resolution
        // fails against the unreachable backend: an enveloped error, not
        // a transport refusal.
        let admitted = client
            .dispatch(authed(&token, call()))
            .await
            .expect("the transport itself must succeed")
            .into_inner();
        assert!(!admitted.error.is_empty());
        assert!(admitted.result_json.is_empty());
    }
}
