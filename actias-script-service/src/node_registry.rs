//! Membership half of the placement store: worker nodes register here and
//! prove liveness by heartbeat. A node that stops beating past the ttl has
//! aged out: liveness reads delete it and its next heartbeat is refused, so
//! it registers again. Object leases will share this store later.

use std::str::FromStr;

use sqlx::types::chrono::{DateTime, Utc};
use sqlx::{Pool, Postgres};
use tonic::{Request, Response, Status};
use uuid::Uuid;

use crate::proto_node_registry::{
    AcquireLeaseRequest, HeartbeatRequest, Lease, ListNodesResponse, Node, NodeRegistration,
    RegisterNodeRequest, ReleaseLeaseRequest, node_registry_service_server::NodeRegistryService,
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
        .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(NodeRegistration {
            node_id: id.to_string(),
            heartbeat_interval_secs: self.heartbeat_interval_secs(),
        }))
    }

    async fn heartbeat(&self, request: Request<HeartbeatRequest>) -> Result<Response<()>, Status> {
        let request = request.get_ref();
        let id = Uuid::from_str(&request.node_id)
            .map_err(|_| Status::invalid_argument("Node id is not a uuid."))?;

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
        .map_err(|e| Status::internal(e.to_string()))?;

        if updated.rows_affected() == 0 {
            return Err(Status::not_found(
                "Node is not registered or has aged out; register again.",
            ));
        }

        Ok(Response::new(()))
    }

    async fn list_nodes(
        &self,
        _request: Request<()>,
    ) -> Result<Response<ListNodesResponse>, Status> {
        // Ageing out is physical: the dead are deleted on read, so the
        // table never accumulates a graveyard.
        sqlx::query("DELETE FROM nodes WHERE last_heartbeat <= $1")
            .bind(self.cutoff())
            .execute(&self.database)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let nodes = sqlx::query_as::<_, DbNode>("SELECT * FROM nodes ORDER BY registered")
            .fetch_all(&self.database)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(ListNodesResponse {
            nodes: nodes.into_iter().map(Node::from).collect(),
        }))
    }

    async fn acquire_lease(
        &self,
        request: Request<AcquireLeaseRequest>,
    ) -> Result<Response<Lease>, Status> {
        let request = request.get_ref();
        let node_id = Uuid::from_str(&request.node_id)
            .map_err(|_| Status::invalid_argument("Node id is not a uuid."))?;

        // A dead holder frees its leases by the same deletion that ages it
        // out; doing it here means a claim never waits for a liveness read.
        sqlx::query("DELETE FROM nodes WHERE last_heartbeat <= $1")
            .bind(self.cutoff())
            .execute(&self.database)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // The conditional claim: exactly one row per object, first insert
        // wins, a re-claim by the current holder is a no-op success.
        let claimed = sqlx::query(
            "INSERT INTO leases (object_id, node_id) VALUES ($1, $2)
             ON CONFLICT (object_id) DO NOTHING",
        )
        .bind(&request.object_id)
        .bind(node_id)
        .execute(&self.database)
        .await
        .map_err(|e| Status::internal(e.to_string()))?;

        let holder: Option<Uuid> =
            sqlx::query_scalar("SELECT node_id FROM leases WHERE object_id = $1")
                .bind(&request.object_id)
                .fetch_optional(&self.database)
                .await
                .map_err(|e| Status::internal(e.to_string()))?;

        let holder = holder.ok_or_else(|| {
            // Claim raced a cascade; the caller simply claims again.
            Status::aborted("The lease was freed mid-claim; try again.")
        })?;

        Ok(Response::new(Lease {
            object_id: request.object_id.clone(),
            node_id: holder.to_string(),
            acquired: claimed.rows_affected() == 1 || holder == node_id,
        }))
    }

    async fn release_lease(
        &self,
        request: Request<ReleaseLeaseRequest>,
    ) -> Result<Response<()>, Status> {
        let request = request.get_ref();
        let node_id = Uuid::from_str(&request.node_id)
            .map_err(|_| Status::invalid_argument("Node id is not a uuid."))?;

        // Only the holder may release; anyone else's release is a no-op,
        // so a laggard cannot free an object out from under its new home.
        sqlx::query("DELETE FROM leases WHERE object_id = $1 AND node_id = $2")
            .bind(&request.object_id)
            .bind(node_id)
            .execute(&self.database)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(()))
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
        // cascade frees the lease, so the claimant now wins.
        backdate(&database, &holder, 46).await;
        let won = acquire(claimant.clone()).await;
        assert!(won.acquired, "an expired lease must be claimable");
        assert_eq!(won.node_id, claimant);
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
            }))
            .await
            .expect("claim answers")
            .into_inner();
        assert!(won.acquired);
    }
}
