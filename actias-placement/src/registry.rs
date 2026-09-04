//! The placement store's rpc surface: worker nodes register here and
//! prove liveness by heartbeat, objects are claimed under leases that
//! live exactly as long as their holder, the instance directory
//! remembers every identity ever claimed, alarms outlive their holders,
//! and a dead node leaves a record of what it held. A node that stops
//! beating past the ttl has aged out: liveness reads delete it and its
//! next heartbeat is refused, so it registers again.
//!
//! Every handler parses the wire and delegates to a
//! [`PlacementStore`]; the store decides nothing about the protocol and
//! the handlers decide nothing about storage.

use std::sync::Arc;

use tonic::{Request, Response, Status};
use uuid::Uuid;

use crate::proto_node_registry::RaiseEpochRequest;
use crate::proto_node_registry::{
    AcquireLeaseRequest, ClearAlarmRequest, CountInstancesRequest, CountInstancesResponse,
    DeleteInstanceRequest, DeleteInstanceResponse, Departure, DeregisterRequest, DueAlarmsRequest,
    DueAlarmsResponse, DueExpiriesRequest, DueExpiriesResponse, GetLeaseRequest, GetNodeRequest,
    HeartbeatRequest, InstanceRef, Lease, ListInstancesRequest, ListInstancesResponse,
    ListNodesResponse, Move, MoveRef, Node, NodeRegistration, ObjectInstance, PurgeInstanceRequest,
    RegisterNodeRequest, ReleaseLeaseRequest, SetAlarmRequest, SetMoveRequest,
    UnfinishedDeletionsResponse, node_registry_service_server::NodeRegistryService,
};
use crate::store::{Identity, NodeRow, PlacementStore, RegistryError, now_ms};

/// A stored instant as the wire spells it.
fn stamp(ms: i64) -> String {
    sqlx::types::chrono::DateTime::<sqlx::types::chrono::Utc>::from_timestamp_millis(ms)
        .map(|at| at.to_string())
        .unwrap_or_default()
}

impl From<NodeRow> for Node {
    fn from(node: NodeRow) -> Self {
        Node {
            node_id: node.id.to_string(),
            address: node.address,
            capabilities: node.capabilities,
            load: node.load.max(0) as u32,
            registered: stamp(node.registered_ms),
            last_heartbeat: stamp(node.last_heartbeat_ms),
        }
    }
}

pub struct NodeRegistry {
    store: Arc<dyn PlacementStore>,
    /// Silence after which a node has aged out.
    ttl_secs: u32,
}

/// Rows one directory page may carry; unreasonable requests clamp here
/// instead of erroring, because a picker retries with whatever it gets.
fn page_size(requested: u32) -> usize {
    if requested == 0 {
        100
    } else {
        requested.min(500) as usize
    }
}

/// The parse every id field shares.
fn uuid(field: &'static str, value: &str) -> Result<Uuid, RegistryError> {
    Uuid::parse_str(value).map_err(|_| RegistryError::InvalidId(field))
}

/// The identity preimage a claim carries, parsed: (scope, script). An
/// identity-less claim (every field empty) is allowed and records nothing
/// in the directory; a partial one is refused.
fn claim_identity(request: &AcquireLeaseRequest) -> Result<Option<Identity>, RegistryError> {
    if request.scope_id.is_empty()
        && request.class.is_empty()
        && request.name.is_empty()
        && request.script_id.is_empty()
    {
        return Ok(None);
    }
    if request.class.is_empty() || request.name.is_empty() {
        return Err(RegistryError::IncompleteIdentity);
    }
    Ok(Some((
        uuid("scope_id", &request.scope_id)?,
        uuid("script_id", &request.script_id)?,
    )))
}

impl NodeRegistry {
    pub fn new(store: Arc<dyn PlacementStore>, ttl_secs: u32) -> Self {
        Self { store, ttl_secs }
    }

    /// Cadence nodes are told to beat at: several beats fit in one ttl, so
    /// a single dropped packet never ages a healthy node out.
    fn heartbeat_interval_secs(&self) -> u32 {
        (self.ttl_secs / 3).max(1)
    }

    /// Oldest heartbeat still considered alive, unix milliseconds.
    fn cutoff_ms(&self) -> i64 {
        now_ms() - i64::from(self.ttl_secs) * 1000
    }

    /// Deletes every node past the ttl. Runs on its own timer, never on
    /// the claim path.
    ///
    /// # Errors
    /// Returns the store's failure; a failed reap leaves the registry
    /// untouched and the next tick tries again.
    pub async fn reap_expired(&self) -> Result<(), RegistryError> {
        self.store.reap_expired(self.cutoff_ms()).await
    }
}

#[tonic::async_trait]
impl NodeRegistryService for NodeRegistry {
    async fn register(
        &self,
        request: Request<RegisterNodeRequest>,
    ) -> Result<Response<NodeRegistration>, Status> {
        let request = request.get_ref();
        let id = self
            .store
            .register(&request.address, &request.capabilities)
            .await?;
        Ok(Response::new(NodeRegistration {
            node_id: id.to_string(),
            heartbeat_interval_secs: self.heartbeat_interval_secs(),
        }))
    }

    async fn heartbeat(&self, request: Request<HeartbeatRequest>) -> Result<Response<()>, Status> {
        let request = request.get_ref();
        let id = uuid("node_id", &request.node_id)?;
        // An aged-out node must not resurrect by beating: the stale one
        // is refused and told to register again.
        let beat = self
            .store
            .heartbeat(id, request.load as i32, self.cutoff_ms())
            .await?;
        if !beat {
            return Err(RegistryError::NodeUnknown.into());
        }
        Ok(Response::new(()))
    }

    async fn list_nodes(
        &self,
        _request: Request<()>,
    ) -> Result<Response<ListNodesResponse>, Status> {
        let nodes = self.store.live_nodes(self.cutoff_ms()).await?;
        Ok(Response::new(ListNodesResponse {
            nodes: nodes.into_iter().map(Node::from).collect(),
        }))
    }

    async fn get_node(&self, request: Request<GetNodeRequest>) -> Result<Response<Node>, Status> {
        let id = uuid("node_id", &request.get_ref().node_id)?;
        let node = self
            .store
            .node(id, self.cutoff_ms())
            .await?
            .ok_or(RegistryError::NoSuchNode)?;
        Ok(Response::new(node.into()))
    }

    async fn acquire_lease(
        &self,
        request: Request<AcquireLeaseRequest>,
    ) -> Result<Response<Lease>, Status> {
        let request = request.get_ref();
        let node = uuid("node_id", &request.node_id)?;
        let identity = claim_identity(request)?;
        Ok(Response::new(
            self.store
                .claim(request, node, identity, self.cutoff_ms())
                .await?,
        ))
    }

    async fn get_lease(
        &self,
        request: Request<GetLeaseRequest>,
    ) -> Result<Response<Lease>, Status> {
        let object_id = request.get_ref().object_id.clone();
        let (holder, epoch) = self
            .store
            .holder(&object_id, self.cutoff_ms())
            .await?
            .ok_or(RegistryError::Unheld)?;
        Ok(Response::new(Lease {
            object_id,
            node_id: holder.to_string(),
            // A lookup never claims; the flag answers "did I get it", and
            // the asker did not ask. Same for freshness.
            acquired: false,
            epoch,
            fresh: false,
            moved_to: String::new(),
        }))
    }

    async fn raise_epoch(
        &self,
        request: Request<RaiseEpochRequest>,
    ) -> Result<Response<Lease>, Status> {
        let request = request.get_ref();
        let node = uuid("node_id", &request.node_id)?;
        let epoch = self
            .store
            .raise_epoch(&request.object_id, node, request.at_least)
            .await?
            .ok_or(RegistryError::Unheld)?;
        Ok(Response::new(Lease {
            object_id: request.object_id.clone(),
            node_id: request.node_id.clone(),
            acquired: true,
            epoch,
            fresh: false,
            moved_to: String::new(),
        }))
    }

    async fn set_move(&self, request: Request<SetMoveRequest>) -> Result<Response<()>, Status> {
        let request = request.get_ref();
        if !actias_common::naming::is_region_token(&request.region) {
            return Err(Status::invalid_argument(format!(
                "'{}' is not a region: 1 to 16 of a-z, 0-9 and '-', not starting with '-'",
                request.region
            )));
        }
        self.store
            .set_move(&request.object_id, &request.region)
            .await?;
        Ok(Response::new(()))
    }

    async fn get_move(&self, request: Request<MoveRef>) -> Result<Response<Move>, Status> {
        let object_id = request.get_ref().object_id.clone();
        let region = self.store.get_move(&object_id).await?.unwrap_or_default();
        Ok(Response::new(Move { object_id, region }))
    }

    async fn clear_move(&self, request: Request<MoveRef>) -> Result<Response<()>, Status> {
        self.store.clear_move(&request.get_ref().object_id).await?;
        Ok(Response::new(()))
    }

    async fn deregister(
        &self,
        request: Request<DeregisterRequest>,
    ) -> Result<Response<()>, Status> {
        let id = uuid("node_id", &request.get_ref().node_id)?;
        self.store.deregister(id).await?;
        Ok(Response::new(()))
    }

    async fn release_lease(
        &self,
        request: Request<ReleaseLeaseRequest>,
    ) -> Result<Response<()>, Status> {
        let request = request.get_ref();
        let node = uuid("node_id", &request.node_id)?;
        // Only the holder may release; anyone else's release is a no-op,
        // so a laggard cannot free an object out from under its new home.
        self.store.release(&request.object_id, node).await?;
        Ok(Response::new(()))
    }

    async fn set_alarm(&self, request: Request<SetAlarmRequest>) -> Result<Response<()>, Status> {
        let request = request.get_ref();
        // One alarm per object, setting replaces: the same shape the
        // object's own persisted row has.
        self.store
            .set_alarm(&request.object_id, &request.own_key, request.due_ms)
            .await?;
        Ok(Response::new(()))
    }

    async fn clear_alarm(
        &self,
        request: Request<ClearAlarmRequest>,
    ) -> Result<Response<()>, Status> {
        self.store.clear_alarm(&request.get_ref().object_id).await?;
        Ok(Response::new(()))
    }

    async fn due_alarms(
        &self,
        request: Request<DueAlarmsRequest>,
    ) -> Result<Response<DueAlarmsResponse>, Status> {
        let request = request.get_ref();
        let alarms = self
            .store
            .due_alarms(request.now_ms, request.limit.clamp(1, 1024) as usize)
            .await?;
        Ok(Response::new(DueAlarmsResponse { alarms }))
    }

    async fn list_instances(
        &self,
        request: Request<ListInstancesRequest>,
    ) -> Result<Response<ListInstancesResponse>, Status> {
        let request = request.get_ref();
        // A listing is one class of one scope, so every identity read is
        // one partition of the store; the classes themselves come from
        // the counts.
        if request.class.is_empty() {
            return Err(Status::invalid_argument(
                "A listing names its class; CountInstances lists them.",
            ));
        }
        // An empty scope list matches nothing: the safe default for a
        // multi-tenant listing, deliberately. An unscoped call must not
        // fall back to every project's objects.
        let scopes: Vec<Uuid> = request
            .project_ids
            .iter()
            .filter_map(|id| Uuid::parse_str(id).ok())
            .collect();
        let limit = page_size(request.page_size);
        let offset = request.page as usize * limit;
        let (total, instances) = self
            .store
            .list_instances(&scopes, &request.class, &request.name_prefix, limit, offset)
            .await?;
        Ok(Response::new(ListInstancesResponse { instances, total }))
    }

    async fn get_instance(
        &self,
        request: Request<InstanceRef>,
    ) -> Result<Response<ObjectInstance>, Status> {
        let request = request.get_ref();
        let scope = uuid("scope_id", &request.scope_id)?;
        let instance = self
            .store
            .instance(scope, &request.class, &request.name)
            .await?
            .ok_or_else(|| Status::not_found("No instance with that identity."))?;
        Ok(Response::new(instance))
    }

    async fn count_instances(
        &self,
        request: Request<CountInstancesRequest>,
    ) -> Result<Response<CountInstancesResponse>, Status> {
        let scopes: Vec<Uuid> = request
            .get_ref()
            .project_ids
            .iter()
            .filter_map(|id| Uuid::parse_str(id).ok())
            .collect();
        let counts = self.store.count_instances(&scopes).await?;
        Ok(Response::new(CountInstancesResponse { counts }))
    }

    async fn delete_instance(
        &self,
        request: Request<DeleteInstanceRequest>,
    ) -> Result<Response<DeleteInstanceResponse>, Status> {
        let request = request.get_ref();
        let scope = uuid("scope_id", &request.scope_id)?;
        let epoch = self
            .store
            .tombstone(
                scope,
                &request.class,
                &request.name,
                &request.object_id,
                request.only_if_expired,
                now_ms(),
            )
            .await?;
        Ok(Response::new(match epoch {
            Some(epoch) => DeleteInstanceResponse {
                tombstoned: true,
                epoch,
            },
            None => DeleteInstanceResponse {
                tombstoned: false,
                epoch: 0,
            },
        }))
    }

    async fn purge_instance(
        &self,
        request: Request<PurgeInstanceRequest>,
    ) -> Result<Response<()>, Status> {
        let request = request.get_ref();
        let scope = uuid("scope_id", &request.scope_id)?;
        self.store
            .purge(scope, &request.class, &request.name, &request.object_id)
            .await?;
        Ok(Response::new(()))
    }

    async fn take_departure(&self, _request: Request<()>) -> Result<Response<Departure>, Status> {
        let Some(departed) = self.store.take_departure().await? else {
            return Ok(Response::new(Departure::default()));
        };
        Ok(Response::new(Departure {
            node_id: departed.node_id.to_string(),
            instances: departed
                .instances
                .into_iter()
                .map(|held| ObjectInstance {
                    scope_id: held.scope_id.to_string(),
                    class: held.class,
                    name: held.name,
                    ..Default::default()
                })
                .collect(),
        }))
    }

    async fn unfinished_deletions(
        &self,
        request: Request<DueExpiriesRequest>,
    ) -> Result<Response<UnfinishedDeletionsResponse>, Status> {
        let request = request.get_ref();
        let rows = self
            .store
            .unfinished_deletions(request.now_ms, request.limit.clamp(1, 1024) as usize)
            .await?;
        Ok(Response::new(UnfinishedDeletionsResponse { rows }))
    }

    async fn rollback_admission(
        &self,
        request: Request<PurgeInstanceRequest>,
    ) -> Result<Response<()>, Status> {
        let request = request.get_ref();
        let scope = uuid("scope_id", &request.scope_id)?;
        self.store
            .rollback_admission(scope, &request.class, &request.name, &request.object_id)
            .await?;
        Ok(Response::new(()))
    }

    async fn due_expiries(
        &self,
        request: Request<DueExpiriesRequest>,
    ) -> Result<Response<DueExpiriesResponse>, Status> {
        let request = request.get_ref();
        let rows = self
            .store
            .due_expiries(request.now_ms, request.limit.clamp(1, 1024) as usize)
            .await?;
        Ok(Response::new(DueExpiriesResponse { rows }))
    }
}

/// The contract as tests: every backend's test module calls each of
/// these against a fresh registry, so a semantic drift between backends
/// is a red suite, not a deployment surprise. Everything goes through
/// the rpcs; silence is a short ttl and a wait, never a poke at a row.
#[cfg(test)]
pub(crate) mod conformance {
    use super::*;
    use std::time::Duration;

    pub async fn register(registry: &NodeRegistry, address: &str) -> String {
        registry
            .register(Request::new(RegisterNodeRequest {
                address: address.to_owned(),
                capabilities: vec!["http".to_owned()],
            }))
            .await
            .expect("node registers")
            .into_inner()
            .node_id
    }

    /// Lets the ttl pass while `alive` keeps beating, so silence and
    /// liveness are both real.
    pub async fn outlive(registry: &NodeRegistry, alive: &[&str], ttl_secs: u32) {
        let until =
            std::time::Instant::now() + Duration::from_millis(u64::from(ttl_secs) * 1000 + 600);
        while std::time::Instant::now() < until {
            for node in alive {
                registry
                    .heartbeat(Request::new(HeartbeatRequest {
                        node_id: (*node).to_owned(),
                        load: 7,
                    }))
                    .await
                    .expect("a live node's heartbeat lands");
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    fn claim(
        node: &str,
        project: Uuid,
        class: &str,
        name: &str,
        script: Uuid,
    ) -> AcquireLeaseRequest {
        AcquireLeaseRequest {
            object_id: format!("hash-{class}-{name}"),
            node_id: node.to_owned(),
            scope_id: project.to_string(),
            class: class.to_owned(),
            name: name.to_owned(),
            script_id: script.to_string(),
            ..Default::default()
        }
    }

    async fn acquire(registry: &NodeRegistry, request: AcquireLeaseRequest) -> Lease {
        registry
            .acquire_lease(Request::new(request))
            .await
            .expect("claim answers")
            .into_inner()
    }

    async fn lookup(registry: &NodeRegistry, object_id: &str) -> Result<Lease, Status> {
        registry
            .get_lease(Request::new(GetLeaseRequest {
                object_id: object_id.to_owned(),
            }))
            .await
            .map(Response::into_inner)
    }

    /// `registry` was built with a ttl of two seconds.
    pub async fn a_dead_node_ages_out_of_the_registry(registry: &NodeRegistry) {
        let live = register(registry, "live:3000").await;
        let dead = register(registry, "dead:3000").await;

        // The dead node falls silent past the ttl; the live one keeps beating.
        outlive(registry, &[&live], 2).await;

        let nodes = registry
            .list_nodes(Request::new(()))
            .await
            .expect("the registry lists")
            .into_inner()
            .nodes;
        assert_eq!(nodes.len(), 1, "only the live node remains: {nodes:?}");
        assert_eq!(nodes[0].node_id, live);
        assert_eq!(nodes[0].address, "live:3000");
        assert_eq!(nodes[0].load, 7);

        // The reap makes the ageing physical; the dead node stays gone
        // and cannot be looked up.
        registry.reap_expired().await.expect("the reap runs");
        let gone = registry
            .get_node(Request::new(GetNodeRequest { node_id: dead }))
            .await;
        assert!(gone.is_err_and(|status| status.code() == tonic::Code::NotFound));
        assert_eq!(
            registry
                .list_nodes(Request::new(()))
                .await
                .expect("lists")
                .into_inner()
                .nodes
                .len(),
            1
        );
    }

    /// `registry` was built with a ttl of one second.
    pub async fn an_aged_out_node_cannot_heartbeat_back_to_life(registry: &NodeRegistry) {
        let node = register(registry, "flaky:3000").await;
        tokio::time::sleep(Duration::from_millis(1600)).await;

        let refused = registry
            .heartbeat(Request::new(HeartbeatRequest {
                node_id: node,
                load: 0,
            }))
            .await;
        let Err(status) = refused else {
            panic!("a heartbeat past the ttl must be refused");
        };
        assert_eq!(status.code(), tonic::Code::NotFound);
    }

    pub async fn a_claim_records_its_identity_in_the_directory(registry: &NodeRegistry) {
        let node = register(registry, "10.0.0.9:80").await;
        let project = Uuid::new_v4();
        let script = Uuid::new_v4();

        acquire(registry, claim(&node, project, "__queue", "jobs", script)).await;
        // A re-claim is a directory no-op, not a duplicate, even when a
        // different script's code touches the shared identity.
        acquire(
            registry,
            claim(&node, project, "__queue", "jobs", Uuid::new_v4()),
        )
        .await;

        let list = || async {
            registry
                .list_instances(Request::new(ListInstancesRequest {
                    project_ids: vec![project.to_string()],
                    class: "__queue".to_owned(),
                    ..Default::default()
                }))
                .await
                .expect("lists")
                .into_inner()
        };
        let listed = list().await;
        assert_eq!(listed.instances.len(), 1, "one identity, once");
        assert_eq!(listed.total, 1);
        assert_eq!(listed.instances[0].name, "jobs");
        assert_eq!(listed.instances[0].class, "__queue");
        assert_eq!(
            listed.instances[0].node_id, node,
            "the listing names the holder"
        );
        assert_eq!(
            listed.instances[0].script_id,
            script.to_string(),
            "declared-by metadata keeps the first claim's owner"
        );

        // The point read answers the same row; an unknown identity is
        // NOT_FOUND rather than a row of zeros.
        let one = registry
            .get_instance(Request::new(InstanceRef {
                scope_id: project.to_string(),
                class: "__queue".to_owned(),
                name: "jobs".to_owned(),
            }))
            .await
            .expect("answers")
            .into_inner();
        assert_eq!(one.script_id, script.to_string());
        let none = registry
            .get_instance(Request::new(InstanceRef {
                scope_id: project.to_string(),
                class: "__queue".to_owned(),
                name: "nobody".to_owned(),
            }))
            .await;
        assert!(none.is_err_and(|status| status.code() == tonic::Code::NotFound));

        // The directory outlives the lease: release frees the object,
        // the identity stays enumerable, now cold.
        registry
            .release_lease(Request::new(ReleaseLeaseRequest {
                object_id: "hash-__queue-jobs".to_owned(),
                node_id: node.clone(),
            }))
            .await
            .expect("releases");
        let survives = list().await;
        assert_eq!(survives.instances.len(), 1);
        assert_eq!(survives.instances[0].node_id, "", "released reads as cold");
    }

    /// `registry` was built with a ttl of two seconds.
    pub async fn a_lease_is_exclusive_until_its_holder_dies(registry: &NodeRegistry) {
        let holder = register(registry, "holder:3000").await;
        let claimant = register(registry, "claimant:3000").await;
        let object = "a".repeat(64);
        let bare = |node: &str| AcquireLeaseRequest {
            object_id: object.clone(),
            node_id: node.to_owned(),
            ..Default::default()
        };

        // First claim wins; re-claiming your own lease stays a success.
        assert!(acquire(registry, bare(&holder)).await.acquired);
        let first = acquire(registry, bare(&holder)).await;
        assert!(first.acquired);

        // A second claimant loses while the holder lives, and is told who
        // holds it. It also reads the incumbent's epoch, because that is
        // the residency's, not the asker's.
        let refused = acquire(registry, bare(&claimant)).await;
        assert!(!refused.acquired);
        assert_eq!(refused.node_id, holder);
        assert_eq!(refused.epoch, first.epoch);

        // The holder falls silent past the ttl while the claimant keeps
        // beating: the claimant now wins, one epoch on.
        outlive(registry, &[&claimant], 2).await;
        let won = acquire(registry, bare(&claimant)).await;
        assert!(won.acquired, "an expired lease must be claimable");
        assert_eq!(won.node_id, claimant);
        assert!(
            won.epoch > first.epoch,
            "a takeover must advance the epoch: {} vs {}",
            won.epoch,
            first.epoch
        );

        // Re-claiming keeps the epoch; the fence only moves on takeover.
        assert_eq!(acquire(registry, bare(&claimant)).await.epoch, won.epoch);
    }

    pub async fn a_deregistered_node_frees_its_leases_immediately(registry: &NodeRegistry) {
        let leaver = register(registry, "leaver:3100").await;
        let heir = register(registry, "heir:3100").await;
        let object = "e".repeat(64);
        let bare = |node: &str| AcquireLeaseRequest {
            object_id: object.clone(),
            node_id: node.to_owned(),
            ..Default::default()
        };

        acquire(registry, bare(&leaver)).await;
        // The ttl is long; only the goodbye can free this today.
        registry
            .deregister(Request::new(DeregisterRequest {
                node_id: leaver.clone(),
            }))
            .await
            .expect("deregisters");

        let won = acquire(registry, bare(&heir)).await;
        assert!(won.acquired, "the goodbye must free the lease at once");
        assert_eq!(won.node_id, heir);
    }

    /// `registry` was built with a ttl of one second.
    pub async fn a_lease_lookup_names_the_holder_without_claiming(registry: &NodeRegistry) {
        let holder = register(registry, "holder:3100").await;
        let object = "c".repeat(64);

        // Nobody holds it yet: the lookup says so instead of inventing a
        // holder, and crucially has not claimed it for anyone.
        let unheld = lookup(registry, &object).await;
        assert!(unheld.is_err_and(|status| status.code() == tonic::Code::NotFound));

        acquire(
            registry,
            AcquireLeaseRequest {
                object_id: object.clone(),
                node_id: holder.clone(),
                ..Default::default()
            },
        )
        .await;
        let lease = lookup(registry, &object)
            .await
            .expect("the holder is readable");
        assert_eq!(lease.node_id, holder);
        assert!(!lease.acquired, "a lookup never grants anything");

        // The holder ages out: the lookup must read unheld again, not
        // route reads at a corpse.
        tokio::time::sleep(Duration::from_millis(1600)).await;
        let freed = lookup(registry, &object).await;
        assert!(
            freed.is_err_and(|status| status.code() == tonic::Code::NotFound),
            "a dead holder must read as unheld"
        );
    }

    /// The gate the directory's reconciliation stands on. The store
    /// folds identities and the index folds its rows in rust, so this
    /// asserts the two spellings agree, and that the fold sees what a
    /// count cannot: one identity swapped for another.
    pub async fn identity_checksums_fold_what_counts_cannot_see(registry: &NodeRegistry) {
        let node = register(registry, "10.0.0.9:80").await;
        let project = Uuid::new_v4();
        let script = Uuid::new_v4();
        let ids = [
            "4a4e19c3d7b123c9d699716b54e8b1127e13d7f5135c10f0ccbd2d4ec2f1a163",
            "18f9afd487df8a82e6dbe8ca930fef6fa5e431e422305ec2623cd6c9d44dd3f6",
            "98631e9a7490b580a26dcdeb18793fff77432272eb5eda36887bf8e4716f7b26",
        ];
        let lot = |index: usize, object_id: &str| AcquireLeaseRequest {
            object_id: object_id.to_owned(),
            node_id: node.clone(),
            scope_id: project.to_string(),
            class: "Auction".to_owned(),
            name: format!("lot-{index}"),
            script_id: script.to_string(),
            ..Default::default()
        };
        for (index, object_id) in ids.iter().enumerate() {
            acquire(registry, lot(index, object_id)).await;
        }

        let fold = || async {
            registry
                .count_instances(Request::new(CountInstancesRequest {
                    project_ids: vec![project.to_string()],
                }))
                .await
                .expect("counts")
                .into_inner()
                .counts
                .into_iter()
                .find(|row| row.class == "Auction")
                .expect("the class is counted")
        };
        let all = fold().await;
        assert_eq!(all.count, 3);
        assert_eq!(
            all.identities,
            actias_common::directory_identity::checksum(ids),
            "the store's fold and the index's rust fold are one value"
        );

        // A tombstoned identity leaves the fold, because the index
        // retires its row: the two must agree on which objects the class
        // has. One arrives in its place.
        let deleted = registry
            .delete_instance(Request::new(DeleteInstanceRequest {
                scope_id: project.to_string(),
                class: "Auction".to_owned(),
                name: "lot-2".to_owned(),
                object_id: ids[2].to_owned(),
                only_if_expired: false,
            }))
            .await
            .expect("delete answers")
            .into_inner();
        assert!(deleted.tombstoned);
        let replacement = "c0ffee11d7b123c9d699716b54e8b1127e13d7f5135c10f0ccbd2d4ec2f1a163";
        acquire(registry, lot(3, replacement)).await;

        let swapped = fold().await;
        assert_eq!(
            swapped.count,
            all.count + 1,
            "the tombstone is still a row until the janitor purges it"
        );
        assert_eq!(
            swapped.identities,
            actias_common::directory_identity::checksum([ids[0], ids[1], replacement]),
            "a swapped identity changes the fold, which is the whole point"
        );
    }

    pub async fn a_seeded_class_of_a_thousand_pages_by_prefix(registry: &NodeRegistry) {
        let node = register(registry, "seeder:3000").await;
        let project = Uuid::new_v4();
        let script = Uuid::new_v4();

        for n in 1..=1000 {
            acquire(
                registry,
                claim(&node, project, "UserCart", &format!("user-{n:05}"), script),
            )
            .await;
        }
        acquire(
            registry,
            claim(&node, project, "Warehouse", "eu-west", script),
        )
        .await;

        let list = |class: &str, prefix: &str, page: u32, page_size: u32| {
            let request = ListInstancesRequest {
                project_ids: vec![project.to_string()],
                class: class.to_owned(),
                name_prefix: prefix.to_owned(),
                page_size,
                page,
            };
            async move {
                registry
                    .list_instances(Request::new(request))
                    .await
                    .expect("lists")
                    .into_inner()
            }
        };

        // The counts answer the rail without touching a single name.
        let counts = registry
            .count_instances(Request::new(CountInstancesRequest {
                project_ids: vec![project.to_string()],
            }))
            .await
            .expect("counts")
            .into_inner()
            .counts;
        assert_eq!(
            counts
                .iter()
                .map(|row| (row.class.as_str(), row.count))
                .collect::<Vec<_>>(),
            vec![("UserCart", 1_000), ("Warehouse", 1)],
        );

        // A prefix narrows a thousand to the ten `user-0042x` names, paged.
        let narrowed = list("UserCart", "user-0042", 0, 4).await;
        assert_eq!(narrowed.total, 10, "user-00420..user-00429");
        assert_eq!(narrowed.instances.len(), 4);
        assert_eq!(narrowed.instances[0].name, "user-00420");
        let last_page = list("UserCart", "user-0042", 2, 4).await;
        assert_eq!(last_page.instances.len(), 2, "10 rows = pages of 4,4,2");
        assert_eq!(last_page.instances[1].name, "user-00429");

        // A wildcard in the prefix is a literal, not a wildcard.
        let literal = list("UserCart", "user-%", 0, 10).await;
        assert_eq!(literal.total, 0, "nobody is named 'user-%...'");

        // The unfiltered listing clamps instead of returning every row.
        let unfiltered = list("UserCart", "", 0, 0).await;
        assert_eq!(unfiltered.total, 1_000);
        assert_eq!(unfiltered.instances.len(), 100, "the default page");

        // A listing is one class; the classes come from the counts.
        let classless = registry
            .list_instances(Request::new(ListInstancesRequest {
                project_ids: vec![project.to_string()],
                ..Default::default()
            }))
            .await
            .expect_err("no class");
        assert_eq!(classless.code(), tonic::Code::InvalidArgument);
    }

    /// `registry` was built with a ttl of one second.
    pub async fn a_due_alarm_outlives_its_holder_and_answers_the_sweep(registry: &NodeRegistry) {
        let holder = register(registry, "holder:3100").await;
        let object = "d".repeat(64);
        acquire(
            registry,
            AcquireLeaseRequest {
                object_id: object.clone(),
                node_id: holder.clone(),
                ..Default::default()
            },
        )
        .await;

        // The holder mirrors a due-in-the-past alarm, then replaces it:
        // one row per object, the latest write wins.
        let arm = |due_ms: i64| SetAlarmRequest {
            object_id: object.clone(),
            own_key: "proj-1/Keeper/watchdog".to_owned(),
            due_ms,
        };
        registry
            .set_alarm(Request::new(arm(1_000)))
            .await
            .expect("arms");
        registry
            .set_alarm(Request::new(arm(2_000)))
            .await
            .expect("re-arms");

        // The holder dies. Its lease frees; the alarm row must not:
        // firing it is now some survivor's job.
        tokio::time::sleep(Duration::from_millis(1600)).await;
        let due_at = |now_ms: i64| async move {
            registry
                .due_alarms(Request::new(DueAlarmsRequest { now_ms, limit: 10 }))
                .await
                .expect("the sweep queries")
                .into_inner()
                .alarms
        };
        let due = due_at(2_000).await;
        assert_eq!(due.len(), 1, "one row per object, latest write");
        assert_eq!(due[0].own_key, "proj-1/Keeper/watchdog");
        assert_eq!(due[0].due_ms, 2_000);
        assert!(due_at(1_999).await.is_empty(), "not due yet");

        // Fired (or cleared): the row goes, the sweep goes quiet.
        registry
            .clear_alarm(Request::new(ClearAlarmRequest {
                object_id: object.clone(),
            }))
            .await
            .expect("clears");
        assert!(due_at(i64::MAX).await.is_empty());
    }

    pub async fn only_the_holder_can_release_a_lease(registry: &NodeRegistry) {
        let holder = register(registry, "holder:3000").await;
        let stranger = register(registry, "stranger:3000").await;
        let object = "b".repeat(64);
        let bare = |node: &str| AcquireLeaseRequest {
            object_id: object.clone(),
            node_id: node.to_owned(),
            ..Default::default()
        };
        acquire(registry, bare(&holder)).await;

        // A stranger's release is a no-op; the holder keeps the lease.
        registry
            .release_lease(Request::new(ReleaseLeaseRequest {
                object_id: object.clone(),
                node_id: stranger.clone(),
            }))
            .await
            .expect("release answers");
        assert!(
            !acquire(registry, bare(&stranger)).await.acquired,
            "a stranger's release must not free it"
        );

        // The holder's release does free it.
        registry
            .release_lease(Request::new(ReleaseLeaseRequest {
                object_id: object.clone(),
                node_id: holder.clone(),
            }))
            .await
            .expect("release answers");
        assert!(acquire(registry, bare(&stranger)).await.acquired);
    }

    pub async fn the_lifecycle_round_trips_through_the_directory(registry: &NodeRegistry) {
        let node = register(registry, "holder:3000").await;
        let project = Uuid::new_v4();
        let script = Uuid::new_v4();
        let doomed = || AcquireLeaseRequest {
            object_id: "hash-doomed".to_owned(),
            node_id: node.clone(),
            scope_id: project.to_string(),
            class: "Session".to_owned(),
            name: "doomed".to_owned(),
            script_id: script.to_string(),
            ..Default::default()
        };

        // A claim with a lifespan stamps expiry and the hash.
        let lease = acquire(
            registry,
            AcquireLeaseRequest {
                expire_secs: 1,
                ..doomed()
            },
        )
        .await;
        assert!(lease.acquired);
        let first_epoch = lease.epoch;

        // Not due yet; then past due, one row; an alarm blocks it again.
        let due = |at: i64| DueExpiriesRequest {
            now_ms: at,
            limit: 16,
        };
        let expiries = |at: i64| async move {
            registry
                .due_expiries(Request::new(due(at)))
                .await
                .expect("due answers")
                .into_inner()
                .rows
        };
        assert!(expiries(now_ms()).await.is_empty(), "not due yet");
        let later = now_ms() + 2_000;
        let past = expiries(later).await;
        assert_eq!(past.len(), 1);
        assert_eq!(past[0].name, "doomed");

        registry
            .set_alarm(Request::new(SetAlarmRequest {
                object_id: "hash-doomed".to_owned(),
                own_key: "Session/doomed".to_owned(),
                due_ms: later + 60_000,
            }))
            .await
            .expect("alarm sets");
        assert!(expiries(later).await.is_empty(), "an alarm blocks expiry");

        // The sweep's guarded tombstone refuses while the alarm stands;
        // an external (unguarded) delete does not.
        let delete = |only_if_expired: bool| async move {
            registry
                .delete_instance(Request::new(DeleteInstanceRequest {
                    scope_id: project.to_string(),
                    class: "Session".to_owned(),
                    name: "doomed".to_owned(),
                    object_id: "hash-doomed".to_owned(),
                    only_if_expired,
                }))
                .await
                .expect("delete answers")
                .into_inner()
        };
        assert!(!delete(true).await.tombstoned);
        let deleted = delete(false).await;
        assert!(deleted.tombstoned);
        assert!(deleted.epoch > first_epoch, "the tombstone bumps the fence");

        // Tombstoned: claims refuse, the listing shows the state, and the
        // unfinished-deletion sweep offers it with a marker above the
        // tombstone.
        let refused_claim = registry.acquire_lease(Request::new(doomed())).await;
        assert!(refused_claim.is_err(), "a deleting identity is not a home");
        let listed = registry
            .list_instances(Request::new(ListInstancesRequest {
                project_ids: vec![project.to_string()],
                class: "Session".to_owned(),
                ..Default::default()
            }))
            .await
            .expect("list answers")
            .into_inner();
        assert_eq!(listed.instances.len(), 1);
        assert!(listed.instances[0].deleted_at_ms > 0);
        let unfinished = registry
            .unfinished_deletions(Request::new(due(now_ms() + 1_000)))
            .await
            .expect("answers")
            .into_inner()
            .rows;
        assert_eq!(unfinished.len(), 1);
        assert_eq!(unfinished[0].name, "doomed");
        assert!(
            unfinished[0].epoch >= deleted.epoch,
            "the marker outranks the tombstone"
        );

        // Purge ends it: the row leaves, the alarm goes, recreation is
        // legal and lands above the bumped epoch.
        registry
            .purge_instance(Request::new(PurgeInstanceRequest {
                scope_id: project.to_string(),
                class: "Session".to_owned(),
                name: "doomed".to_owned(),
                object_id: "hash-doomed".to_owned(),
            }))
            .await
            .expect("purge answers");
        assert!(
            registry
                .unfinished_deletions(Request::new(due(now_ms() + 1_000)))
                .await
                .expect("answers")
                .into_inner()
                .rows
                .is_empty(),
            "purged rows leave the sweep"
        );
        let recreated = acquire(registry, doomed()).await;
        assert!(recreated.acquired);
        assert!(
            recreated.epoch > deleted.epoch,
            "a fresh life starts above every old fence"
        );

        // An admission rollback unwinds the claim it refused: the lease
        // goes with the identity.
        let junk = acquire(
            registry,
            AcquireLeaseRequest {
                object_id: "hash-junk".to_owned(),
                name: "junk".to_owned(),
                ..doomed()
            },
        )
        .await;
        assert!(junk.fresh && junk.epoch > 0);
        registry
            .rollback_admission(Request::new(PurgeInstanceRequest {
                scope_id: project.to_string(),
                class: "Session".to_owned(),
                name: "junk".to_owned(),
                object_id: "hash-junk".to_owned(),
            }))
            .await
            .expect("rollback answers");
        assert!(
            lookup(registry, "hash-junk")
                .await
                .is_err_and(|status| status.code() == tonic::Code::NotFound),
            "a refused fresh name leaves no lease behind"
        );

        registry
            .rollback_admission(Request::new(PurgeInstanceRequest {
                scope_id: project.to_string(),
                class: "Session".to_owned(),
                name: "doomed".to_owned(),
                object_id: "hash-doomed".to_owned(),
            }))
            .await
            .expect("rollback answers");
        // The name that lived is unwound too, and its next life still
        // cannot land under the tombstone.
        let relived = acquire(registry, doomed()).await;
        assert!(
            relived.epoch > recreated.epoch,
            "every life claims above the last: {} vs {}",
            relived.epoch,
            recreated.epoch
        );
    }

    pub async fn a_reborn_name_claims_above_its_tombstone(registry: &NodeRegistry) {
        let node = register(registry, "10.0.0.9:80").await;
        let project = Uuid::new_v4();
        let lot = || AcquireLeaseRequest {
            object_id: "hash-lot42".to_owned(),
            node_id: node.clone(),
            scope_id: project.to_string(),
            class: "Auction".to_owned(),
            name: "lot42".to_owned(),
            script_id: Uuid::new_v4().to_string(),
            ..Default::default()
        };

        let first = acquire(registry, lot()).await;
        assert!(first.acquired);

        // Destruction: tombstone plus epoch bump, then the janitor's
        // purge removes lease and directory row.
        let deleted = registry
            .delete_instance(Request::new(DeleteInstanceRequest {
                scope_id: project.to_string(),
                class: "Auction".to_owned(),
                name: "lot42".to_owned(),
                object_id: "hash-lot42".to_owned(),
                only_if_expired: false,
            }))
            .await
            .expect("delete answers")
            .into_inner();
        assert!(deleted.tombstoned);
        assert!(
            deleted.epoch > first.epoch,
            "the tombstone outranks the life before it"
        );
        registry
            .purge_instance(Request::new(PurgeInstanceRequest {
                scope_id: project.to_string(),
                class: "Auction".to_owned(),
                name: "lot42".to_owned(),
                object_id: "hash-lot42".to_owned(),
            }))
            .await
            .expect("purge answers");

        // The purge leaves no lease behind, and the recreation outranks
        // the tombstone, or the reborn object is invisible to every
        // directory query forever.
        assert!(
            lookup(registry, "hash-lot42")
                .await
                .is_err_and(|status| status.code() == tonic::Code::NotFound),
            "churn must leave no lease behind"
        );
        let reborn = acquire(registry, lot()).await;
        assert!(reborn.acquired);
        assert!(
            reborn.epoch > deleted.epoch,
            "rebirth must claim above the tombstone: {} vs {}",
            reborn.epoch,
            deleted.epoch
        );
    }

    /// The wake path's answer to a marker at or above the claim: the
    /// holder's epoch is raised above it, the identity remembers, and
    /// nobody else may raise it.
    pub async fn an_epoch_is_raised_above_a_marker(registry: &NodeRegistry) {
        let holder = register(registry, "holder:3000").await;
        let stranger = register(registry, "stranger:3000").await;
        let project = Uuid::new_v4();
        let script = Uuid::new_v4();
        let lease = acquire(registry, claim(&holder, project, "Room", "a", script)).await;
        assert!(lease.acquired);
        let raise = |node: &str, at_least: u64| {
            let node = node.to_owned();
            async move {
                registry
                    .raise_epoch(Request::new(RaiseEpochRequest {
                        object_id: "hash-Room-a".to_owned(),
                        node_id: node,
                        at_least,
                    }))
                    .await
            }
        };

        // Below what is held: a no-op that answers the standing epoch.
        let same = raise(&holder, lease.epoch - 1)
            .await
            .expect("answers")
            .into_inner();
        assert_eq!(same.epoch, lease.epoch);

        // Above: raised to it, and the lookup agrees.
        let target = lease.epoch + 1_000_000;
        let raised = raise(&holder, target).await.expect("raises").into_inner();
        assert_eq!(raised.epoch, target);
        assert_eq!(
            lookup(registry, "hash-Room-a").await.expect("held").epoch,
            target
        );

        // A stranger cannot raise what it does not hold.
        let refused = raise(&stranger, target + 1).await;
        assert!(refused.is_err_and(|status| status.code() == tonic::Code::NotFound));

        // The identity remembers: its next life claims above the raise.
        registry
            .release_lease(Request::new(ReleaseLeaseRequest {
                object_id: "hash-Room-a".to_owned(),
                node_id: holder.clone(),
            }))
            .await
            .expect("releases");
        let again = acquire(registry, claim(&stranger, project, "Room", "a", script)).await;
        assert!(again.acquired);
        assert!(again.epoch > target, "{} vs {target}", again.epoch);
    }

    /// `registry` was built with a ttl of two seconds.
    /// A forwarding row stands in for a lease: a claim here answers the
    /// region and takes nothing, the row reads back, and clearing it
    /// makes the next claim an ordinary one.
    pub async fn a_moved_object_is_claimed_by_its_forwarding_row(registry: &NodeRegistry) {
        let node = register(registry, "10.0.0.1:3100").await;
        let object_id = "b".repeat(64);
        let claim = || AcquireLeaseRequest {
            object_id: object_id.clone(),
            node_id: node.clone(),
            ..Default::default()
        };

        assert_eq!(
            registry
                .get_move(Request::new(MoveRef {
                    object_id: object_id.clone(),
                }))
                .await
                .expect("answers")
                .into_inner()
                .region,
            ""
        );
        let refused = registry
            .set_move(Request::new(SetMoveRequest {
                object_id: object_id.clone(),
                region: "EU".to_owned(),
            }))
            .await
            .expect_err("not a token");
        assert_eq!(refused.code(), tonic::Code::InvalidArgument);
        registry
            .set_move(Request::new(SetMoveRequest {
                object_id: object_id.clone(),
                region: "ap-south".to_owned(),
            }))
            .await
            .expect("moves");

        let lease = registry
            .acquire_lease(Request::new(claim()))
            .await
            .expect("answers")
            .into_inner();
        assert!(!lease.acquired);
        assert_eq!(lease.moved_to, "ap-south");
        assert!(
            registry
                .get_lease(Request::new(GetLeaseRequest {
                    object_id: object_id.clone(),
                }))
                .await
                .is_err(),
            "no lease was taken"
        );
        assert_eq!(
            registry
                .get_move(Request::new(MoveRef {
                    object_id: object_id.clone(),
                }))
                .await
                .expect("answers")
                .into_inner()
                .region,
            "ap-south"
        );

        registry
            .clear_move(Request::new(MoveRef {
                object_id: object_id.clone(),
            }))
            .await
            .expect("clears");
        registry
            .clear_move(Request::new(MoveRef {
                object_id: object_id.clone(),
            }))
            .await
            .expect("clearing twice is nothing");
        let lease = registry
            .acquire_lease(Request::new(claim()))
            .await
            .expect("answers")
            .into_inner();
        assert!(lease.acquired && lease.moved_to.is_empty());
    }

    pub async fn departures_capture_the_flag_and_the_leases(registry: &NodeRegistry) {
        let steady = register(registry, "steady:3000").await;
        let doomed = register(registry, "doomed:3000").await;
        let project = Uuid::new_v4();
        let script = Uuid::new_v4();

        acquire(registry, claim(&doomed, project, "Room", "a", script)).await;
        acquire(registry, claim(&doomed, project, "Room", "b", script)).await;
        acquire(registry, claim(&steady, project, "Room", "c", script)).await;

        let take = || async {
            registry
                .take_departure(Request::new(()))
                .await
                .expect("take answers")
                .into_inner()
        };
        assert!(take().await.node_id.is_empty(), "nothing has departed");

        // Graceful goodbye: drained, so the sweep has nothing to repair.
        registry
            .deregister(Request::new(DeregisterRequest {
                node_id: steady.clone(),
            }))
            .await
            .expect("deregister answers");
        assert!(
            take().await.node_id.is_empty(),
            "a drained departure carries no repair"
        );

        // Unclean death via the reaper: undrained, both identities
        // captured before the leases went, handed out exactly once.
        tokio::time::sleep(Duration::from_millis(2600)).await;
        registry.reap_expired().await.expect("the reap runs");
        let departed = take().await;
        assert_eq!(departed.node_id, doomed);
        let mut held: Vec<_> = departed.instances.iter().map(|i| i.name.clone()).collect();
        held.sort();
        assert_eq!(held, vec!["a", "b"]);
        assert!(
            take().await.node_id.is_empty(),
            "a departure is handed out once"
        );
    }
}

/// The postgres backend's tests: the conformance suite over a real
/// postgres with the migrations applied.
#[cfg(test)]
mod postgres_tests {
    use super::conformance;
    use super::*;
    use sqlx::postgres::PgPoolOptions;
    use testcontainers_modules::{
        postgres::Postgres as PostgresImage,
        testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner},
    };

    async fn registry(ttl_secs: u32) -> (NodeRegistry, ContainerAsync<PostgresImage>) {
        let postgres = PostgresImage::default()
            .with_tag("17-alpine")
            .start()
            .await
            .expect("postgres starts");
        let port = postgres
            .get_host_port_ipv4(5432)
            .await
            .expect("postgres port is published");
        let database = PgPoolOptions::new()
            .connect(&format!(
                "postgresql://postgres:postgres@127.0.0.1:{port}/postgres"
            ))
            .await
            .expect("postgres accepts connections");
        sqlx::migrate!("./migrations")
            .run(&database)
            .await
            .expect("migrations apply");
        (
            NodeRegistry::new(
                Arc::new(crate::postgres::PostgresStore::new(database)),
                ttl_secs,
            ),
            postgres,
        )
    }

    macro_rules! conforms {
        ($name:ident, $ttl:expr) => {
            #[tokio::test]
            async fn $name() {
                let (registry, _guard) = registry($ttl).await;
                conformance::$name(&registry).await;
            }
        };
    }

    conforms!(a_dead_node_ages_out_of_the_registry, 2);
    conforms!(an_aged_out_node_cannot_heartbeat_back_to_life, 1);
    conforms!(a_claim_records_its_identity_in_the_directory, 60);
    conforms!(a_lease_is_exclusive_until_its_holder_dies, 2);
    conforms!(a_deregistered_node_frees_its_leases_immediately, 3600);
    conforms!(a_lease_lookup_names_the_holder_without_claiming, 1);
    conforms!(identity_checksums_fold_what_counts_cannot_see, 60);
    conforms!(a_seeded_class_of_a_thousand_pages_by_prefix, 60);
    conforms!(a_due_alarm_outlives_its_holder_and_answers_the_sweep, 1);
    conforms!(only_the_holder_can_release_a_lease, 45);
    conforms!(the_lifecycle_round_trips_through_the_directory, 45);
    conforms!(a_reborn_name_claims_above_its_tombstone, 60);
    conforms!(an_epoch_is_raised_above_a_marker, 60);
    conforms!(departures_capture_the_flag_and_the_leases, 2);
    conforms!(a_moved_object_is_claimed_by_its_forwarding_row, 60);
}

/// The scylla backend's tests: the conformance suite over a real scylla
/// with the migrations applied, plus the container plumbing only this
/// backend needs. One container per test, serialized; scylla in
/// developer mode starts in seconds.
#[cfg(test)]
mod scylla_tests {
    use super::conformance;
    use super::*;
    use scylla::client::session_builder::SessionBuilder;
    use scylla::errors::TranslationError;
    use scylla::policies::address_translator::{AddressTranslator, UntranslatedPeer};
    use serial_test::serial;
    use std::net::SocketAddr;
    use testcontainers::runners::AsyncRunner;
    use testcontainers::{ContainerAsync, GenericImage, ImageExt, core::WaitFor};

    /// Routes every discovered peer to one address: the container
    /// advertises its bridge ip, but the driver pairs that ip with the
    /// mapped host port, an address nothing listens on.
    struct EverythingIsHere(SocketAddr);

    #[async_trait::async_trait]
    impl AddressTranslator for EverythingIsHere {
        async fn translate_address(
            &self,
            _peer: &UntranslatedPeer,
        ) -> Result<SocketAddr, TranslationError> {
            Ok(self.0)
        }
    }

    async fn registry(ttl_secs: u32) -> (NodeRegistry, ContainerAsync<GenericImage>) {
        // The store's failures reach tracing, not the wire; a failing
        // test wants to see them.
        let _ = actias_common::setup_tracing();
        let container = GenericImage::new("scylladb/scylla", "6.2")
            .with_wait_for(WaitFor::message_on_stderr("serving"))
            .with_cmd([
                "--smp",
                "1",
                "--developer-mode",
                "1",
                "--overprovisioned",
                "1",
            ])
            .start()
            .await
            .expect("scylla starts");
        let port = container
            .get_host_port_ipv4(9042)
            .await
            .expect("cql port is published");
        let addr: SocketAddr = ([127, 0, 0, 1], port).into();
        let builder = || {
            SessionBuilder::new()
                .known_node(addr.to_string())
                .address_translator(Arc::new(EverythingIsHere(addr)))
        };
        // Readiness is a served query, not a connection: a fresh session
        // can still have a broken pool while shards come up.
        let mut session = None;
        for _ in 0..60 {
            if let Ok(s) = builder().build().await
                && s.query_unpaged("SELECT release_version FROM system.local", ())
                    .await
                    .is_ok()
            {
                session = Some(s);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        let session = session.expect("scylla accepts connections");
        // The real migrator, twice: the second run must find everything
        // recorded and change nothing.
        crate::migrate::apply(&session, "datacenter1", 1)
            .await
            .expect("migrations apply");
        crate::migrate::apply(&session, "datacenter1", 1)
            .await
            .expect("migrations are re-runnable");
        let data_session = builder()
            .use_keyspace("placement", true)
            .build()
            .await
            .expect("scylla accepts a data session");
        let store = crate::scylla::ScyllaStore::new(data_session, "local".to_owned()).await;
        (NodeRegistry::new(Arc::new(store), ttl_secs), container)
    }

    macro_rules! conforms {
        ($name:ident, $ttl:expr) => {
            #[tokio::test]
            #[serial(containers)]
            async fn $name() {
                let (registry, _guard) = registry($ttl).await;
                conformance::$name(&registry).await;
            }
        };
    }

    conforms!(a_dead_node_ages_out_of_the_registry, 2);
    conforms!(an_aged_out_node_cannot_heartbeat_back_to_life, 1);
    conforms!(a_claim_records_its_identity_in_the_directory, 60);
    conforms!(a_lease_is_exclusive_until_its_holder_dies, 2);
    conforms!(a_deregistered_node_frees_its_leases_immediately, 3600);
    conforms!(a_lease_lookup_names_the_holder_without_claiming, 1);
    conforms!(identity_checksums_fold_what_counts_cannot_see, 60);
    conforms!(a_seeded_class_of_a_thousand_pages_by_prefix, 60);
    conforms!(a_due_alarm_outlives_its_holder_and_answers_the_sweep, 1);
    conforms!(only_the_holder_can_release_a_lease, 45);
    conforms!(the_lifecycle_round_trips_through_the_directory, 45);
    conforms!(a_reborn_name_claims_above_its_tombstone, 60);
    conforms!(an_epoch_is_raised_above_a_marker, 60);
    conforms!(departures_capture_the_flag_and_the_leases, 2);
    conforms!(a_moved_object_is_claimed_by_its_forwarding_row, 60);
}
