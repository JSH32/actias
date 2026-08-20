//! Membership half of the placement store: worker nodes register here and
//! prove liveness by heartbeat. A node that stops beating past the ttl has
//! aged out: liveness reads delete it and its next heartbeat is refused, so
//! it registers again. Object leases will share this store later.

use std::str::FromStr;

use actias_common::thiserror;
use sqlx::types::chrono::{DateTime, Utc};
use sqlx::{Pool, Postgres};
use tonic::{Request, Response, Status};
use uuid::Uuid;

use crate::proto_node_registry::{
    AcquireLeaseRequest, GetLeaseRequest, GetNodeRequest, HeartbeatRequest, Lease,
    ListInstancesRequest, ListInstancesResponse, ListNodesResponse, Node, NodeRegistration,
    ObjectInstance, RegisterNodeRequest, ReleaseLeaseRequest,
    node_registry_service_server::NodeRegistryService,
};

/// One registry row.
#[derive(sqlx::FromRow)]
struct DbNode {
    id: Uuid,
    address: String,
    capabilities: Vec<String>,
    load: i32,
    registered: DateTime<Utc>,
    last_heartbeat: DateTime<Utc>,
}

impl From<DbNode> for Node {
    fn from(node: DbNode) -> Self {
        Node {
            node_id: node.id.to_string(),
            address: node.address,
            capabilities: node.capabilities,
            load: node.load.max(0) as u32,
            registered: node.registered.to_string(),
            last_heartbeat: node.last_heartbeat.to_string(),
        }
    }
}

pub struct NodeRegistry {
    database: Pool<Postgres>,
    /// Silence after which a node has aged out.
    ttl_secs: u32,
}

/// What can fail inside the registry. The [`From`] impl below is the one
/// place deciding what the wire sees; raw store detail stops at tracing.
#[derive(thiserror::Error, Debug)]
enum RegistryError {
    #[error("placement store query failed: {0}")]
    Store(#[from] sqlx::Error),
    #[error("'{0}' is not a uuid")]
    InvalidId(&'static str),
    #[error("claim identity is incomplete")]
    IncompleteIdentity,
    #[error("node unknown or aged out")]
    NodeUnknown,
    #[error("no live node with that id")]
    NoSuchNode,
    #[error("nobody holds the object")]
    Unheld,
    #[error("claim raced a cascade")]
    ClaimRaced,
}

impl From<RegistryError> for Status {
    fn from(error: RegistryError) -> Self {
        match error {
            RegistryError::Store(source) => {
                actias_common::tracing::error!(error = %source, "placement store query failed");
                Status::internal("The placement store failed.")
            }
            RegistryError::InvalidId(field) => {
                Status::invalid_argument(format!("'{field}' is not a uuid."))
            }
            RegistryError::IncompleteIdentity => Status::invalid_argument(
                "A claim identity carries scope, class, name and script, or none of them.",
            ),
            RegistryError::NodeUnknown => {
                Status::not_found("Node is not registered or has aged out; register again.")
            }
            RegistryError::NoSuchNode => Status::not_found("No live node with that id."),
            RegistryError::Unheld => Status::not_found("Nobody holds that object."),
            // The caller simply claims again.
            RegistryError::ClaimRaced => {
                Status::aborted("The lease was freed mid-claim; try again.")
            }
        }
    }
}

/// The identity preimage a claim carries, parsed: (scope, script). An
/// identity-less claim (every field empty) is allowed and records nothing
/// in the directory; a partial one is refused.
fn claim_identity(request: &AcquireLeaseRequest) -> Result<Option<(Uuid, Uuid)>, RegistryError> {
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
    let scope =
        Uuid::from_str(&request.scope_id).map_err(|_| RegistryError::InvalidId("scope_id"))?;
    let script =
        Uuid::from_str(&request.script_id).map_err(|_| RegistryError::InvalidId("script_id"))?;
    Ok(Some((scope, script)))
}

impl NodeRegistry {
    pub fn new(database: Pool<Postgres>, ttl_secs: u32) -> Self {
        Self { database, ttl_secs }
    }

    /// Cadence nodes are told to beat at: several beats fit in one ttl, so
    /// a single dropped packet never ages a healthy node out.
    fn heartbeat_interval_secs(&self) -> u32 {
        (self.ttl_secs / 3).max(1)
    }

    /// Oldest heartbeat still considered alive.
    fn cutoff(&self) -> DateTime<Utc> {
        Utc::now() - std::time::Duration::from_secs(self.ttl_secs.into())
    }

    /// Deletes every node past the ttl. Ageing out is physical, so the
    /// table never accumulates a graveyard, and lease expiry is the same
    /// deletion through the cascade.
    async fn reap(&self) -> Result<(), RegistryError> {
        sqlx::query("DELETE FROM nodes WHERE last_heartbeat <= $1")
            .bind(self.cutoff())
            .execute(&self.database)
            .await?;
        Ok(())
    }

    /// The conditional claim and everything it settles: directory record,
    /// holder, epoch.
    async fn claim(&self, request: &AcquireLeaseRequest) -> Result<Lease, RegistryError> {
        let node_id =
            Uuid::from_str(&request.node_id).map_err(|_| RegistryError::InvalidId("node_id"))?;
        let identity = claim_identity(request)?;

        // A dead holder frees its leases by the same deletion that ages it
        // out; doing it here means a claim never waits for a liveness read.
        self.reap().await?;

        // The conditional claim: exactly one row per object, first insert
        // wins, a re-claim by the current holder is a no-op success.
        let claimed = sqlx::query(
            "INSERT INTO leases (object_id, node_id) VALUES ($1, $2)
             ON CONFLICT (object_id) DO NOTHING",
        )
        .bind(&request.object_id)
        .bind(node_id)
        .execute(&self.database)
        .await?;

        // The claim carries its preimage; the directory keeps it so the
        // data stays enumerable after the declaring revision is gone.
        if let Some((scope_id, script_id)) = identity {
            sqlx::query(
                "INSERT INTO object_instances (scope_id, class, name, script_id)
                 VALUES ($1, $2, $3, $4)
                 ON CONFLICT (scope_id, class, name) DO NOTHING",
            )
            .bind(scope_id)
            .bind(&request.class)
            .bind(&request.name)
            .bind(script_id)
            .execute(&self.database)
            .await?;
        }

        let holder: Option<Uuid> =
            sqlx::query_scalar("SELECT node_id FROM leases WHERE object_id = $1")
                .bind(&request.object_id)
                .fetch_optional(&self.database)
                .await?;
        let holder = holder.ok_or(RegistryError::ClaimRaced)?;
        let acquired = claimed.rows_affected() == 1 || holder == node_id;

        // A fresh claim advances the object's epoch, which never resets:
        // it is the fence storage shipping writes into its manifests.
        let epoch: i64 = if claimed.rows_affected() == 1 {
            sqlx::query_scalar(
                "INSERT INTO object_epochs (object_id) VALUES ($1)
                 ON CONFLICT (object_id)
                 DO UPDATE SET epoch = object_epochs.epoch + 1
                 RETURNING epoch",
            )
            .bind(&request.object_id)
            .fetch_one(&self.database)
            .await?
        } else {
            sqlx::query_scalar("SELECT epoch FROM object_epochs WHERE object_id = $1")
                .bind(&request.object_id)
                .fetch_optional(&self.database)
                .await?
                .unwrap_or(1)
        };

        Ok(Lease {
            object_id: request.object_id.clone(),
            node_id: holder.to_string(),
            acquired,
            epoch: epoch.max(1) as u64,
        })
    }
}

#[tonic::async_trait]
impl NodeRegistryService for NodeRegistry {
    async fn register(
        &self,
        request: Request<RegisterNodeRequest>,
    ) -> Result<Response<NodeRegistration>, Status> {
        let request = request.get_ref();

        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO nodes (address, capabilities) VALUES ($1, $2) RETURNING id",
        )
        .bind(&request.address)
        .bind(&request.capabilities)
        .fetch_one(&self.database)
        .await
        .map_err(RegistryError::Store)?;

        Ok(Response::new(NodeRegistration {
            node_id: id.to_string(),
            heartbeat_interval_secs: self.heartbeat_interval_secs(),
        }))
    }

    async fn heartbeat(&self, request: Request<HeartbeatRequest>) -> Result<Response<()>, Status> {
        let request = request.get_ref();
        let id =
            Uuid::from_str(&request.node_id).map_err(|_| RegistryError::InvalidId("node_id"))?;

        // An aged-out node must not resurrect by beating: only a row still
        // inside the ttl accepts the update, so the stale one is refused
        // and told to register again.
        let updated = sqlx::query(
            "UPDATE nodes SET last_heartbeat = now(), load = $2
             WHERE id = $1 AND last_heartbeat > $3",
        )
        .bind(id)
        .bind(request.load as i32)
        .bind(self.cutoff())
        .execute(&self.database)
        .await
        .map_err(RegistryError::Store)?;

        if updated.rows_affected() == 0 {
            return Err(RegistryError::NodeUnknown.into());
        }

        Ok(Response::new(()))
    }

    async fn list_nodes(
        &self,
        _request: Request<()>,
    ) -> Result<Response<ListNodesResponse>, Status> {
        self.reap().await?;

        let nodes = sqlx::query_as::<_, DbNode>("SELECT * FROM nodes ORDER BY registered")
            .fetch_all(&self.database)
            .await
            .map_err(RegistryError::Store)?;

        Ok(Response::new(ListNodesResponse {
            nodes: nodes.into_iter().map(Node::from).collect(),
        }))
    }

    async fn get_node(&self, request: Request<GetNodeRequest>) -> Result<Response<Node>, Status> {
        let id = Uuid::from_str(&request.get_ref().node_id)
            .map_err(|_| RegistryError::InvalidId("node_id"))?;

        let node = sqlx::query_as::<_, DbNode>(
            "SELECT * FROM nodes WHERE id = $1 AND last_heartbeat > $2",
        )
        .bind(id)
        .bind(self.cutoff())
        .fetch_optional(&self.database)
        .await
        .map_err(RegistryError::Store)?
        .ok_or(RegistryError::NoSuchNode)?;

        Ok(Response::new(node.into()))
    }

    async fn acquire_lease(
        &self,
        request: Request<AcquireLeaseRequest>,
    ) -> Result<Response<Lease>, Status> {
        Ok(Response::new(self.claim(request.get_ref()).await?))
    }

    async fn get_lease(
        &self,
        request: Request<GetLeaseRequest>,
    ) -> Result<Response<Lease>, Status> {
        let object_id = request.get_ref().object_id.clone();

        // A dead holder must read as unheld, exactly as a claim would
        // treat it; leases free through the same deletion cascade.
        self.reap().await?;

        let holder: Option<Uuid> =
            sqlx::query_scalar("SELECT node_id FROM leases WHERE object_id = $1")
                .bind(&object_id)
                .fetch_optional(&self.database)
                .await
                .map_err(RegistryError::Store)?;
        let holder = holder.ok_or(RegistryError::Unheld)?;

        let epoch: i64 = sqlx::query_scalar("SELECT epoch FROM object_epochs WHERE object_id = $1")
            .bind(&object_id)
            .fetch_optional(&self.database)
            .await
            .map_err(RegistryError::Store)?
            .unwrap_or(1);

        Ok(Response::new(Lease {
            object_id,
            node_id: holder.to_string(),
            // A lookup never claims; the flag answers "did I get it", and
            // the asker did not ask.
            acquired: false,
            epoch: epoch.max(1) as u64,
        }))
    }

    async fn release_lease(
        &self,
        request: Request<ReleaseLeaseRequest>,
    ) -> Result<Response<()>, Status> {
        let request = request.get_ref();
        let node_id =
            Uuid::from_str(&request.node_id).map_err(|_| RegistryError::InvalidId("node_id"))?;

        // Only the holder may release; anyone else's release is a no-op,
        // so a laggard cannot free an object out from under its new home.
        sqlx::query("DELETE FROM leases WHERE object_id = $1 AND node_id = $2")
            .bind(&request.object_id)
            .bind(node_id)
            .execute(&self.database)
            .await
            .map_err(RegistryError::Store)?;

        Ok(Response::new(()))
    }

    async fn list_instances(
        &self,
        request: Request<ListInstancesRequest>,
    ) -> Result<Response<ListInstancesResponse>, Status> {
        let project_ids: Vec<Uuid> = request
            .get_ref()
            .project_ids
            .iter()
            .filter_map(|id| Uuid::from_str(id).ok())
            .collect();

        // Cron rows scope to their script, so a project listing never
        // matches them: only resource identities surface here.
        let rows: Vec<(Uuid, String, String, Uuid, i64)> = sqlx::query_as(
            "SELECT scope_id, class, name, script_id,
                    (EXTRACT(EPOCH FROM created) * 1000)::BIGINT
             FROM object_instances WHERE scope_id = ANY($1)
             ORDER BY class, name",
        )
        .bind(&project_ids)
        .fetch_all(&self.database)
        .await
        .map_err(RegistryError::Store)?;

        Ok(Response::new(ListInstancesResponse {
            instances: rows
                .into_iter()
                .map(
                    |(scope_id, class, name, script_id, created_ms)| ObjectInstance {
                        scope_id: scope_id.to_string(),
                        class,
                        name,
                        script_id: script_id.to_string(),
                        created_ms,
                    },
                )
                .collect(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::postgres::PgPoolOptions;
    use testcontainers_modules::{
        postgres::Postgres as PostgresImage,
        testcontainers::{ImageExt, runners::AsyncRunner},
    };

    /// A registry over a real postgres with the migrations applied.
    async fn registry(
        ttl_secs: u32,
    ) -> (
        NodeRegistry,
        Pool<Postgres>,
        testcontainers_modules::testcontainers::ContainerAsync<PostgresImage>,
    ) {
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
            NodeRegistry::new(database.clone(), ttl_secs),
            database,
            postgres,
        )
    }

    async fn register(registry: &NodeRegistry, address: &str) -> String {
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

    /// Pushes a node's heartbeat into the past, standing in for silence.
    async fn backdate(database: &Pool<Postgres>, node_id: &str, seconds: i64) {
        sqlx::query(
            "UPDATE nodes SET last_heartbeat = now() - make_interval(secs => $2) WHERE id = $1",
        )
        .bind(Uuid::from_str(node_id).expect("node id is a uuid"))
        .bind(seconds as f64)
        .execute(database)
        .await
        .expect("backdate applies");
    }

    #[tokio::test]
    async fn a_dead_node_ages_out_of_the_registry() {
        let (registry, database, _guard) = registry(45).await;

        let live = register(&registry, "live:3000").await;
        let dead = register(&registry, "dead:3000").await;

        // The dead node falls silent past the ttl; the live one keeps beating.
        backdate(&database, &dead, 46).await;
        registry
            .heartbeat(Request::new(HeartbeatRequest {
                node_id: live.clone(),
                load: 7,
            }))
            .await
            .expect("a live node's heartbeat lands");

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

        // Ageing out is deletion, not filtering: the row is gone.
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM nodes")
            .fetch_one(&database)
            .await
            .expect("count reads");
        assert_eq!(rows, 1);
    }

    #[tokio::test]
    async fn an_aged_out_node_cannot_heartbeat_back_to_life() {
        let (registry, database, _guard) = registry(45).await;

        let node = register(&registry, "flaky:3000").await;
        backdate(&database, &node, 46).await;

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

    #[tokio::test]
    async fn a_claim_records_its_identity_in_the_directory() {
        let (registry, _database, _container) = registry(60).await;
        let node = register(&registry, "10.0.0.9:80").await;
        let project = Uuid::new_v4();
        let script = Uuid::new_v4();

        let claim = |name: &str, script: Uuid| AcquireLeaseRequest {
            object_id: format!("hash-{name}"),
            node_id: node.clone(),
            scope_id: project.to_string(),
            class: "__queue".to_owned(),
            name: name.to_owned(),
            script_id: script.to_string(),
        };
        registry
            .acquire_lease(Request::new(claim("jobs", script)))
            .await
            .expect("claims");
        // A re-claim is a directory no-op, not a duplicate, even when a
        // different script's code touches the shared identity.
        registry
            .acquire_lease(Request::new(claim("jobs", Uuid::new_v4())))
            .await
            .expect("re-claims");

        let listed = registry
            .list_instances(Request::new(ListInstancesRequest {
                project_ids: vec![project.to_string()],
            }))
            .await
            .expect("lists")
            .into_inner();
        assert_eq!(listed.instances.len(), 1, "one identity, once");
        assert_eq!(listed.instances[0].name, "jobs");
        assert_eq!(listed.instances[0].class, "__queue");
        assert_eq!(
            listed.instances[0].script_id,
            script.to_string(),
            "declared-by metadata keeps the first claim's owner"
        );

        // The directory outlives the lease: release frees the object,
        // the identity stays enumerable.
        registry
            .release_lease(Request::new(ReleaseLeaseRequest {
                object_id: "hash-jobs".to_owned(),
                node_id: node.clone(),
            }))
            .await
            .expect("releases");
        let survives = registry
            .list_instances(Request::new(ListInstancesRequest {
                project_ids: vec![project.to_string()],
            }))
            .await
            .expect("lists again")
            .into_inner();
        assert_eq!(survives.instances.len(), 1);
    }

    #[tokio::test]
    async fn a_lease_is_exclusive_until_its_holder_dies() {
        let (registry, database, _guard) = registry(45).await;

        let holder = register(&registry, "holder:3000").await;
        let claimant = register(&registry, "claimant:3000").await;
        let object = "a".repeat(64);

        let acquire = |node: String| {
            let registry = &registry;
            let object = object.clone();
            async move {
                registry
                    .acquire_lease(Request::new(AcquireLeaseRequest {
                        object_id: object,
                        node_id: node,
                        ..Default::default()
                    }))
                    .await
                    .expect("claim answers")
                    .into_inner()
            }
        };

        // First claim wins; re-claiming your own lease stays a success.
        assert!(acquire(holder.clone()).await.acquired);
        assert!(acquire(holder.clone()).await.acquired);

        // A second claimant loses while the holder lives, and is told who
        // holds it.
        let refused = acquire(claimant.clone()).await;
        assert!(!refused.acquired);
        assert_eq!(refused.node_id, holder);

        // The holder falls silent past the ttl: its node ages out and the
        // cascade frees the lease, so the claimant now wins, one epoch on.
        backdate(&database, &holder, 46).await;
        let won = acquire(claimant.clone()).await;
        assert!(won.acquired, "an expired lease must be claimable");
        assert_eq!(won.node_id, claimant);
        assert_eq!(won.epoch, 2, "a takeover must advance the epoch");

        // Re-claiming keeps the epoch; the fence only moves on takeover.
        assert_eq!(acquire(claimant).await.epoch, 2);
    }

    #[tokio::test]
    async fn a_lease_lookup_names_the_holder_without_claiming() {
        let (registry, database, _guard) = registry(45).await;

        let holder = register(&registry, "holder:3100").await;
        let object = "c".repeat(64);

        // Nobody holds it yet: the lookup says so instead of inventing a
        // holder, and crucially has not claimed it for anyone.
        let unheld = registry
            .get_lease(Request::new(GetLeaseRequest {
                object_id: object.clone(),
            }))
            .await;
        let Err(status) = unheld else {
            panic!("an unheld object must read as NOT_FOUND");
        };
        assert_eq!(status.code(), tonic::Code::NotFound);

        registry
            .acquire_lease(Request::new(AcquireLeaseRequest {
                object_id: object.clone(),
                node_id: holder.clone(),
                ..Default::default()
            }))
            .await
            .expect("claims");

        let lease = registry
            .get_lease(Request::new(GetLeaseRequest {
                object_id: object.clone(),
            }))
            .await
            .expect("the holder is readable")
            .into_inner();
        assert_eq!(lease.node_id, holder);
        assert!(!lease.acquired, "a lookup never grants anything");

        // The holder ages out: the lookup must read unheld again, not
        // route reads at a corpse.
        backdate(&database, &holder, 46).await;
        let freed = registry
            .get_lease(Request::new(GetLeaseRequest { object_id: object }))
            .await;
        assert!(
            freed.is_err_and(|status| status.code() == tonic::Code::NotFound),
            "a dead holder must read as unheld"
        );
    }

    #[tokio::test]
    async fn only_the_holder_can_release_a_lease() {
        let (registry, _database, _guard) = registry(45).await;

        let holder = register(&registry, "holder:3000").await;
        let stranger = register(&registry, "stranger:3000").await;
        let object = "b".repeat(64);

        registry
            .acquire_lease(Request::new(AcquireLeaseRequest {
                object_id: object.clone(),
                node_id: holder.clone(),
                ..Default::default()
            }))
            .await
            .expect("claims");

        // A stranger's release is a no-op; the holder keeps the lease.
        registry
            .release_lease(Request::new(ReleaseLeaseRequest {
                object_id: object.clone(),
                node_id: stranger.clone(),
            }))
            .await
            .expect("release answers");
        let refused = registry
            .acquire_lease(Request::new(AcquireLeaseRequest {
                object_id: object.clone(),
                node_id: stranger.clone(),
                ..Default::default()
            }))
            .await
            .expect("claim answers")
            .into_inner();
        assert!(!refused.acquired, "a stranger's release must not free it");

        // The holder's release does free it.
        registry
            .release_lease(Request::new(ReleaseLeaseRequest {
                object_id: object.clone(),
                node_id: holder.clone(),
            }))
            .await
            .expect("release answers");
        let won = registry
            .acquire_lease(Request::new(AcquireLeaseRequest {
                object_id: object,
                node_id: stranger,
                ..Default::default()
            }))
            .await
            .expect("claim answers")
            .into_inner();
        assert!(won.acquired);
    }
}
