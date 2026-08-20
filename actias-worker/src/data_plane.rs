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
use actias_worker_core::proto::worker_data::{CallResult, ObjectCall, ReadRequest, ReadValue};

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
) -> Result<WorkerDataClient<Channel>, String> {
    if let Some(channel) = state.peers.get(address).await {
        return Ok(WorkerDataClient::new(channel));
    }
    let endpoint = Channel::from_shared(format!("http://{address}"))
        .map_err(|_| "The peer's address is not routable.".to_owned())?;
    let channel = endpoint.connect_lazy();
    state
        .peers
        .insert(address.to_owned(), channel.clone())
        .await;
    Ok(WorkerDataClient::new(channel))
}

/// The read a request asks for; `sql` outranks `messages` outranks the
/// class's default overview.
// A Status-sized Err is the rpc surface's contract, not a choice.
#[allow(clippy::result_large_err)]
fn stats_read(request: &ReadRequest) -> Result<PlatformRead, Status> {
    match request.sql.clone() {
        Some(sql) => Ok(PlatformRead::Query { sql }),
        None if request.messages => Ok(PlatformRead::QueueMessages),
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
            },
            Err(error) => CallResult {
                result_json: String::new(),
                error,
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
