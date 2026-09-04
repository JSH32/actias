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
    CallResult, ConnectionList, ConnectionQuery, ConnectionRow, DirectoryEntry, DirectoryPage,
    DirectoryQuery, DirectoryRebuild, DirectoryRebuilt, GenerationPart, InboxBatch, InboxEdge,
    InboxReceipts, ObjectCall, ReadRequest, ReadValue, ReceiveBatch, ReceiveEntry, ReceiveOutcome,
    ReceiveReceipts, ReplicaChunk, ReplicaInfo, ReplicaQuery, ShellOutcome, ShellRun, VisitEntry,
    VisitPage, WalAppend, WalAppended, WatermarkInfo, WatermarkQuery, generation_part,
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
        return Ok(sized(WorkerDataClient::new(
            actias_worker_core::plain_grpc(channel),
        )));
    }
    // A peer inside the region is host:port; another region's ingress
    // carries its own scheme (https, behind the operator's TLS).
    let uri = if address.contains("://") {
        address.to_owned()
    } else {
        format!("http://{address}")
    };
    let endpoint =
        Channel::from_shared(uri).map_err(|_| "The peer's address is not routable.".to_owned())?;
    let channel = endpoint.connect_lazy();
    state
        .peers
        .insert(address.to_owned(), channel.clone())
        .await;
    Ok(sized(WorkerDataClient::new(
        actias_worker_core::plain_grpc(channel),
    )))
}

/// Nothing larger than one chunk of a base, plus its framing, crosses
/// the data plane in one message: a generation travels as a stream of
/// parts, and a copy as a stream of chunks.
pub const PEER_MESSAGE_BYTES: usize = 4 << 20;

fn sized(
    client: WorkerDataClient<actias_worker_core::Grpc>,
) -> WorkerDataClient<actias_worker_core::Grpc> {
    client
        .max_decoding_message_size(PEER_MESSAGE_BYTES)
        .max_encoding_message_size(PEER_MESSAGE_BYTES)
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
        None if request.state => Ok(PlatformRead::StateStore),
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
            // This node's own replica copy is one flight behind the owner;
            // the store's is a ttl behind.
            match self.state.replica_store.read_copy(&key.object_id()).await {
                Ok(Some(copy)) => Some(copy),
                // No copy here, or one that could not be laid (a sweep
                // took the generation under it): the store's copy serves.
                _ => fresh_replica_file(&self.state, &key.object_id())
                    .await
                    .map_err(Status::invalid_argument)?,
            }
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
    type FetchReplicaStream = std::pin::Pin<
        Box<dyn futures::Stream<Item = Result<ReplicaChunk, Status>> + Send + 'static>,
    >;

    async fn append_wal(
        &self,
        request: Request<WalAppend>,
    ) -> Result<Response<WalAppended>, Status> {
        let append = request.into_inner();
        let outcome = self
            .state
            .replica_store
            .append(
                &append.object_id,
                append.epoch,
                append.base,
                append.offset,
                &append.bytes,
                append.covered,
            )
            .await
            .map_err(|error| {
                actias_common::tracing::warn!(%error, object_id = append.object_id, "replica append failed");
                Status::internal("The replica could not append.")
            })?;
        Ok(Response::new(WalAppended {
            length: outcome.length,
            applied: outcome.applied,
            refusal: outcome.refusal,
        }))
    }

    async fn list_connections(
        &self,
        request: Request<ConnectionQuery>,
    ) -> Result<Response<ConnectionList>, Status> {
        let query = request.into_inner();
        let mut rows: Vec<ConnectionRow> = self
            .state
            .connections
            .list()
            .into_iter()
            .filter(|row| query.project_id.is_empty() || row.project_id == query.project_id)
            .map(|row| ConnectionRow {
                id: row.id,
                connection_class: row.connection_class,
                class: row.class,
                name: row.name,
                direction: row.direction.as_str().to_owned(),
                peer: row.peer.unwrap_or_default(),
                node: row.node,
                project_id: row.project_id,
                script_id: row.script_id,
                opened_at_ms: row.opened_at_ms,
                status: row.status.as_str().to_owned(),
                follows: row.follows as u32,
            })
            .collect();
        if !query.local_only {
            // Every other live node answers for itself; a node that does
            // not answer in time lists nothing rather than failing the
            // page.
            let own = self
                .state
                .node_identity
                .read()
                .ok()
                .and_then(|guard| guard.clone());
            for node in crate::directory::rebuild::live_nodes(&self.state).await {
                if own.as_deref() == Some(node.as_str()) {
                    continue;
                }
                let Ok(address) = crate::directory::route::address_of(&self.state, &node).await
                else {
                    continue;
                };
                if address == self.state.node_address {
                    continue;
                }
                let Ok(mut client) = peer_client(&self.state, &address).await else {
                    continue;
                };
                let ask = ConnectionQuery {
                    project_id: query.project_id.clone(),
                    local_only: true,
                };
                if let Ok(Ok(reply)) = tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    client.list_connections(authed(&self.state.internal_token, ask)),
                )
                .await
                {
                    rows.extend(reply.into_inner().connections);
                }
            }
        }
        rows.sort_by_key(|row| std::cmp::Reverse(row.opened_at_ms));
        Ok(Response::new(ConnectionList { connections: rows }))
    }

    async fn lay_generation(
        &self,
        request: Request<tonic::Streaming<GenerationPart>>,
    ) -> Result<Response<WalAppended>, Status> {
        use futures::StreamExt;
        let mut parts = request.into_inner();
        let header = match parts.message().await? {
            Some(GenerationPart {
                part: Some(generation_part::Part::Header(header)),
            }) => header,
            _ => return Err(Status::invalid_argument("A lay starts with its header.")),
        };
        let object_id = header.object_id.clone();
        // Chunks until done; a stream that ends without it is an error
        // the store turns into a forgotten copy.
        let chunks = futures::stream::unfold((parts, false), |(mut parts, done)| async move {
            if done {
                return None;
            }
            match parts.message().await {
                Ok(Some(GenerationPart {
                    part: Some(generation_part::Part::Chunk(chunk)),
                })) => Some((Ok((chunk.index, chunk.bytes)), (parts, false))),
                Ok(Some(GenerationPart {
                    part: Some(generation_part::Part::Done(_)),
                })) => None,
                Ok(Some(_)) => Some((Err("a lay carries a header once".to_owned()), (parts, true))),
                Ok(None) => Some((Err("the lay ended before done".to_owned()), (parts, true))),
                Err(status) => Some((Err(status.message().to_owned()), (parts, true))),
            }
        });
        let outcome = self
            .state
            .replica_store
            .lay(
                &object_id,
                crate::objects::replica::LayHeader {
                    epoch: header.epoch,
                    base: header.base,
                    from_list: header.from_list,
                    base_len: header.base_len,
                    chunks: header.chunks,
                },
                chunks.boxed(),
            )
            .await
            .map_err(|error| {
                actias_common::tracing::warn!(%error, object_id, "replica lay failed");
                Status::internal("The replica could not lay the generation.")
            })?;
        Ok(Response::new(WalAppended {
            length: outcome.length,
            applied: outcome.applied,
            refusal: outcome.refusal,
        }))
    }

    async fn watermark(
        &self,
        request: Request<WatermarkQuery>,
    ) -> Result<Response<WatermarkInfo>, Status> {
        let object_id = request.into_inner().object_id;
        let state = self
            .state
            .ship_states
            .lock()
            .expect("no poisoned lock")
            .get(&object_id)
            .cloned();
        let Some(state) = state else {
            return Ok(Response::new(WatermarkInfo::default()));
        };
        let (residency_epoch, released) = (state.epoch(), state.released());
        Ok(Response::new(match released {
            Some((epoch, base, length)) => WatermarkInfo {
                held: true,
                epoch,
                base,
                length,
                released: true,
            },
            // Resident, nothing released in this residency yet: no copy
            // can be vouched for, and the reader asks the owner itself.
            None => WatermarkInfo {
                held: true,
                epoch: residency_epoch,
                released: false,
                ..Default::default()
            },
        }))
    }

    async fn replica_state(
        &self,
        request: Request<ReplicaQuery>,
    ) -> Result<Response<ReplicaInfo>, Status> {
        let query = request.into_inner();
        let info = self
            .state
            .replica_store
            .state(&query.object_id, query.epoch, query.base, query.fence_to)
            .await
            .map_err(|error| {
                actias_common::tracing::warn!(%error, object_id = query.object_id, "replica fence could not be written");
                Status::internal("The replica could not record the fence.")
            })?;
        Ok(Response::new(ReplicaInfo {
            held: info.held,
            length: info.length,
            fence: info.fence,
        }))
    }

    async fn fetch_replica(
        &self,
        request: Request<ReplicaQuery>,
    ) -> Result<Response<Self::FetchReplicaStream>, Status> {
        let query = request.into_inner();
        // The fence rises here too: handing a copy over is part of a
        // takeover, and the old owner must not extend it afterwards.
        self.state
            .replica_store
            .state(&query.object_id, query.epoch, query.base, query.fence_to)
            .await
            .map_err(|error| {
                actias_common::tracing::warn!(%error, object_id = query.object_id, "replica fence could not be written");
                Status::internal("The replica could not record the fence.")
            })?;
        let Some(copy) = self
            .state
            .replica_store
            .fetch(&query.object_id, query.epoch, query.base)
            .await
        else {
            return Err(Status::not_found(
                "This node does not hold that generation.",
            ));
        };
        const CHUNK: u64 = 1 << 20;
        let base_chunks = copy.base_len.div_ceil(CHUNK);
        let wal_chunks = copy.wal_len.div_ceil(CHUNK);
        // Read from disk as the stream is polled; nothing holds the copy.
        let object_id = query.object_id.clone();
        let chunks = futures::stream::unfold(0u64, move |i| {
            let copy = std::sync::Arc::new((
                copy.base.clone(),
                copy.base_len,
                copy.wal.clone(),
                copy.wal_len,
            ));
            let object_id = object_id.clone();
            async move {
                if i >= base_chunks + wal_chunks {
                    return None;
                }
                let (path, len, at, is_base) = if i < base_chunks {
                    (copy.0.clone(), copy.1, i * CHUNK, true)
                } else {
                    (copy.2.clone(), copy.3, (i - base_chunks) * CHUNK, false)
                };
                let read = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
                    use std::os::unix::fs::FileExt;
                    let file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
                    let mut bytes = vec![0u8; (len - at).min(CHUNK) as usize];
                    file.read_exact_at(&mut bytes, at)
                        .map_err(|e| e.to_string())?;
                    Ok(bytes)
                })
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r);
                let item = match read {
                    Ok(bytes) => Ok(if is_base {
                        ReplicaChunk {
                            base: actias_worker_core::proto::Bytes::from(bytes),
                            wal: actias_worker_core::proto::Bytes::new(),
                        }
                    } else {
                        ReplicaChunk {
                            base: actias_worker_core::proto::Bytes::new(),
                            wal: actias_worker_core::proto::Bytes::from(bytes),
                        }
                    }),
                    Err(error) => {
                        actias_common::tracing::warn!(%error, object_id, "replica copy could not be read");
                        Err(Status::internal("The replica copy could not be read."))
                    }
                };
                Some((item, i + 1))
            }
        });
        Ok(Response::new(Box::pin(chunks)))
    }

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
                        // resolved here is who runs the code, not who
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
                moved_to: String::new(),
            },
            // The object was born here and lives elsewhere: the caller
            // remembers the region and forwards there once.
            Err(crate::routing::RouteError::Moved { region }) => CallResult {
                result_json: String::new(),
                error: format!("Object lives in region {region}; this region cannot serve it."),
                wrong_home: false,
                moved_to: region,
            },
            // The typed refusal crosses the wire as its own flag, so the
            // sender re-resolves instead of parsing message text.
            Err(crate::routing::RouteError::WrongHome { holder }) => CallResult {
                result_json: String::new(),
                error: format!("Object is homed on {holder}; this node cannot serve it."),
                wrong_home: true,
                moved_to: String::new(),
            },
            Err(crate::routing::RouteError::Failed(error)) => CallResult {
                result_json: String::new(),
                error,
                wrong_home: false,
                moved_to: String::new(),
            },
        }))
    }

    /// One shell chunk, run in a fresh vm under the session's grants.
    ///
    /// The chunk becomes a revision of its own: an entry that registers
    /// it as a handler, a synthetic script scoped to the project, and a
    /// contract holding exactly the grants the api derived from what the
    /// operator may open. Declarations are allowed inside the handler,
    /// because a shell binds resources as it goes; everything else is the
    /// ordinary runtime, budgeted like a request, with the object router
    /// and the directory lister a script would have. Prints are captured
    /// and returned with the value rather than published to a channel
    /// nobody is tailing.
    async fn run_shell(
        &self,
        request: Request<ShellRun>,
    ) -> Result<Response<ShellOutcome>, Status> {
        let run = request.into_inner();
        if run.scope_id.is_empty() {
            return Err(Status::invalid_argument("The shell run names no project."));
        }
        let started = std::time::Instant::now();
        let outcome = crate::shell_run::run(&self.state, run).await;
        Ok(Response::new(match outcome {
            Ok(mut done) => {
                done.wall_ms = started.elapsed().as_millis() as u64;
                done
            }
            Err(error) => ShellOutcome {
                value_json: String::new(),
                output: Vec::new(),
                error,
                work: 0,
                wall_ms: started.elapsed().as_millis() as u64,
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

    /// One publisher's due events for the connections this node hosts:
    /// the events arrive once, each edge names its slice (topic, seq
    /// watermark, filter), and this walks the local registry to fan
    /// out, reporting back whoever is gone. The publisher sent one
    /// call for everything here instead of one per socket, and one
    /// copy of each event instead of one per socket.
    async fn deliver_inbox(
        &self,
        request: Request<InboxBatch>,
    ) -> Result<Response<InboxReceipts>, Status> {
        use actias_worker_core::streams::filter_matches;

        let batch = request.into_inner();
        let events =
            serde_json::from_str::<Vec<serde_json::Value>>(&batch.events_json).unwrap_or_default();
        let mut gone = Vec::new();
        for edge in batch.edges {
            let filter = (!edge.filter_json.is_empty())
                .then(|| serde_json::from_str::<serde_json::Value>(&edge.filter_json).ok())
                .flatten();
            for event in &events {
                if event["topic"].as_str() != Some(edge.topic.as_str())
                    || event["seq"].as_i64().unwrap_or(0) <= edge.after
                    || !filter_matches(filter.as_ref(), &event["data"])
                {
                    continue;
                }
                let item = actias_worker_core::connections::InboxItem::Event {
                    topic: edge.topic.clone(),
                    from_class: event["from_class"].as_str().unwrap_or_default().to_owned(),
                    from_name: event["from_name"].as_str().unwrap_or_default().to_owned(),
                    data: event["data"].clone(),
                };
                if self
                    .state
                    .connections
                    .deliver(&edge.connection, item)
                    .is_err()
                {
                    if !gone.contains(&edge.connection) {
                        gone.push(edge.connection.clone());
                    }
                    break;
                }
            }
        }
        Ok(Response::new(InboxReceipts { gone }))
    }

    /// One publisher's due events for the durable followers this node
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

    async fn list_directory(
        &self,
        request: Request<DirectoryQuery>,
    ) -> Result<Response<DirectoryPage>, Status> {
        // A hop is served here, whoever this node is: the reader is a
        // preference the first node applied, and two hops never happen.
        let hop = crate::directory::route::is_hop(&request);
        let request = request.into_inner();
        let (class, query) = directory_request(&request).map_err(Status::invalid_argument)?;
        // A refused field (unknown, or still building) is the caller's
        // mistake or their answer to wait, never an internal failure.
        let (page, building) = if hop {
            self.state
                .directory_gauges
                .count(&self.state.directory_gauges.served_for_peer);
            let page = crate::directory::read::list(&self.state, &class, query)
                .await
                .map_err(Status::invalid_argument)?;
            let building = crate::directory::read::building(&self.state, &class).await;
            (page, building)
        } else {
            crate::directory::route::list(&self.state, &class, query)
                .await
                .map_err(Status::invalid_argument)?
        };
        Ok(Response::new(DirectoryPage {
            entries: page
                .entries
                .into_iter()
                .map(|entry| DirectoryEntry {
                    name: entry.name,
                    object_id: entry.object_id,
                    fields: entry
                        .fields
                        .into_iter()
                        .map(|(name, value)| (name, crate::directory::query::value_to_json(&value)))
                        .collect(),
                })
                .collect(),
            cursor: page.cursor,
            building,
        }))
    }

    async fn visit_directory(
        &self,
        request: Request<DirectoryQuery>,
    ) -> Result<Response<VisitPage>, Status> {
        let hop = crate::directory::route::is_hop(&request);
        let request = request.into_inner();
        let (class, query) = directory_request(&request).map_err(Status::invalid_argument)?;
        let (page, building) = if hop {
            self.state
                .directory_gauges
                .count(&self.state.directory_gauges.served_for_peer);
            let page = crate::directory::visit::visit(&self.state, &class, query)
                .await
                .map_err(Status::invalid_argument)?;
            let building = crate::directory::read::building(&self.state, &class).await;
            (page, building)
        } else {
            crate::directory::route::visit(&self.state, &class, query)
                .await
                .map_err(Status::invalid_argument)?
        };
        Ok(Response::new(VisitPage {
            entries: page
                .entries
                .into_iter()
                .map(|served| VisitEntry {
                    entry: Some(DirectoryEntry {
                        name: served.entry.name,
                        object_id: served.entry.object_id,
                        fields: served
                            .entry
                            .fields
                            .into_iter()
                            .map(|(name, value)| {
                                (name, crate::directory::query::value_to_json(&value))
                            })
                            .collect(),
                    }),
                    unverified: served.unverified,
                    reason: served.reason.unwrap_or_default(),
                })
                .collect(),
            cursor: page.cursor,
            building,
        }))
    }

    async fn rebuild_directory(
        &self,
        request: Request<DirectoryRebuild>,
    ) -> Result<Response<DirectoryRebuilt>, Status> {
        let hop = crate::directory::route::is_hop(&request);
        let request = request.into_inner();
        if request.scope_id.is_empty() || request.class.is_empty() {
            return Err(Status::invalid_argument(
                "a rebuild names a project and a class.",
            ));
        }
        let class = crate::directory::sync::ClassKey {
            scope_id: request.scope_id.clone(),
            class: request.class.clone(),
        };

        let rebuilt = crate::directory::rebuild::rebuild_on_demand(&self.state, &class)
            .await
            .map_err(Status::unavailable)?;

        // The class is held by the node that folds it, so the rebuild
        // is that node's to run: one hop, never two, so a request that
        // already hopped and still lost answers "held elsewhere" rather
        // than bouncing.
        let Some(rebuilt) = rebuilt else {
            if !hop
                && let Some(forwarded) =
                    crate::directory::rebuild::forward_to_holder(&self.state, &class, request).await
            {
                return Ok(Response::new(forwarded));
            }
            return Ok(Response::new(DirectoryRebuilt {
                held: false,
                ..Default::default()
            }));
        };
        Ok(Response::new(DirectoryRebuilt {
            live: rebuilt.live as u64,
            rows: rebuilt.rows as u64,
            without_row: rebuilt.without_row as u64,
            tombstones: rebuilt.tombstones as u64,
            held: true,
        }))
    }
}

/// The wire query as the kernel takes it: one translation shared by the
/// listing and the verified read, so the two cannot drift on what a
/// query means.
#[allow(clippy::type_complexity)]
fn directory_request(
    request: &DirectoryQuery,
) -> Result<
    (
        crate::directory::sync::ClassKey,
        actias_worker_core::directory::overlay::Query,
    ),
    String,
> {
    let class = crate::directory::sync::ClassKey {
        scope_id: request.scope_id.clone(),
        class: request.class.clone(),
    };
    let query = actias_worker_core::directory::overlay::Query {
        where_: crate::directory::query::where_from_proto(request.r#where.as_ref())?,
        order: request
            .order
            .iter()
            .map(|entry| actias_worker_core::directory::predicate::Order {
                field: entry.field.clone(),
                descending: entry.descending,
            })
            .collect(),
        limit: request.limit.clamp(1, crate::directory::read::MAX_LIMIT),
        cursor: request.cursor.clone(),
    };
    Ok((class, query))
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
                state: false,
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
        move |node: String, batch: actias_worker_core::streams::NodeInbox| {
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
                        let mut gone: Vec<String> = batch
                            .edges
                            .into_iter()
                            .map(|edge| edge.connection)
                            .collect();
                        gone.dedup();
                        return Ok(gone);
                    }
                    Err(e) => {
                        return Err(format!("the follower's node could not be resolved: {e}"));
                    }
                };
                let mut client = peer_client(&state, &resolved.address).await?;
                let edges = batch
                    .edges
                    .into_iter()
                    .map(|edge| InboxEdge {
                        connection: edge.connection,
                        topic: edge.topic,
                        after: edge.after,
                        filter_json: edge
                            .filter
                            .map(|filter| filter.to_string())
                            .unwrap_or_default(),
                    })
                    .collect();
                let receipts = client
                    .deliver_inbox(authed(
                        &state.internal_token,
                        InboxBatch {
                            events_json: serde_json::Value::Array(batch.events).to_string(),
                            edges,
                        },
                    ))
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
            state: false,
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

    /// The receiving half of node-grouped fan-out: events arrive once
    /// and each edge takes its own slice by topic, watermark and
    /// filter; a connection nobody holds is reported gone exactly
    /// once, however many edges it rode in on.
    #[tokio::test(flavor = "multi_thread")]
    async fn deliver_inbox_slices_the_shared_events_per_edge() {
        use actias_worker_core::connections::InboxItem;

        let state = state_with(empty_caches());
        let (inbox_tx, mut inbox_rx) = actias_worker_core::connections::inbox();
        state.connections.register("conn#here", inbox_tx);
        let service = WorkerDataService::new(state);

        let events = serde_json::json!([
            { "seq": 1, "topic": "news", "from_class": "Hub", "from_name": "town",
              "data": { "kind": "sport" } },
            { "seq": 2, "topic": "news", "from_class": "Hub", "from_name": "town",
              "data": { "kind": "weather" } },
            { "seq": 3, "topic": "noise", "from_class": "Hub", "from_name": "town",
              "data": { "kind": "static" } },
        ]);
        let receipts = service
            .deliver_inbox(Request::new(InboxBatch {
                events_json: events.to_string(),
                edges: vec![
                    // Heard seq 1 already; the filter drops nothing.
                    InboxEdge {
                        connection: "conn#here".into(),
                        topic: "news".into(),
                        after: 1,
                        filter_json: String::new(),
                    },
                    // A filter that matches nothing this batch holds.
                    InboxEdge {
                        connection: "conn#here".into(),
                        topic: "noise".into(),
                        after: 0,
                        filter_json: serde_json::json!({ "kind": "melody" }).to_string(),
                    },
                    // Two edges of a connection nobody registered.
                    InboxEdge {
                        connection: "conn#gone".into(),
                        topic: "news".into(),
                        after: 0,
                        filter_json: String::new(),
                    },
                    InboxEdge {
                        connection: "conn#gone".into(),
                        topic: "noise".into(),
                        after: 0,
                        filter_json: String::new(),
                    },
                ],
            }))
            .await
            .expect("the batch is served")
            .into_inner();

        assert_eq!(receipts.gone, vec!["conn#gone"], "gone once, not per edge");

        let heard = tokio::time::timeout(std::time::Duration::from_secs(2), inbox_rx.next())
            .await
            .expect("the slice arrives")
            .expect("inbox open");
        match heard {
            InboxItem::Event { topic, data, .. } => {
                assert_eq!(topic, "news");
                assert_eq!(data["kind"], "weather", "seq 1 was already heard");
            }
            other => panic!("not an event: {other:?}"),
        }
        // Nothing else may arrive: the filtered noise event stayed out.
        let quiet =
            tokio::time::timeout(std::time::Duration::from_millis(200), inbox_rx.next()).await;
        assert!(quiet.is_err(), "exactly one event was due: {quiet:?}");
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
