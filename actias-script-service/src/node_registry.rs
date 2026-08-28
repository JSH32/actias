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
    AcquireLeaseRequest, AlarmRow, ClassCount, ClearAlarmRequest, CountInstancesRequest,
    CountInstancesResponse, DeleteInstanceRequest, DeleteInstanceResponse, DeregisterRequest,
    DueAlarmsRequest, DueAlarmsResponse, DueExpiriesRequest, DueExpiriesResponse, ExpiryRow,
    GetLeaseRequest, GetNodeRequest, HeartbeatRequest, Lease, ListInstancesRequest,
    ListInstancesResponse, ListNodesResponse, Node, NodeRegistration, ObjectInstance,
    PurgeInstanceRequest, RegisterNodeRequest, ReleaseLeaseRequest, SetAlarmRequest,
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
pub enum RegistryError {
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
    #[error("the object is being deleted")]
    Deleting,
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
            RegistryError::Deleting => Status::failed_precondition("The object is being deleted."),
            RegistryError::ClaimRaced => {
                Status::aborted("The lease was freed mid-claim; try again.")
            }
        }
    }
}

/// A prefix made safe for `LIKE`: its wildcards become literals, so a
/// user typing `%` searches for `%`.
fn like_prefix(prefix: &str) -> String {
    let mut escaped = String::with_capacity(prefix.len() + 1);
    for character in prefix.chars() {
        if matches!(character, '%' | '_' | '\\') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped.push('%');
    escaped
}

/// Rows one directory page may carry; unreasonable requests clamp here
/// instead of erroring, because a picker retries with whatever it gets.
fn page_size(requested: u32) -> i64 {
    if requested == 0 {
        100
    } else {
        i64::from(requested.min(500))
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
    /// deletion through the cascade. Runs on its own timer, never on the
    /// claim path.
    pub async fn reap_expired(&self) -> Result<(), RegistryError> {
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

        // A tombstoned identity refuses claims until the janitor finishes:
        // deletion in progress is not a home. Recreation becomes legal the
        // moment the directory row is purged.
        if let Some((scope_id, _)) = identity {
            let deleting: Option<bool> = sqlx::query_scalar(
                "SELECT deleted_at IS NOT NULL FROM object_instances
                 WHERE scope_id = $1 AND class = $2 AND name = $3",
            )
            .bind(scope_id)
            .bind(&request.class)
            .bind(&request.name)
            .fetch_optional(&self.database)
            .await?;
            if deleting == Some(true) {
                return Err(RegistryError::Deleting);
            }
        }

        // The conditional claim: exactly one row per object, first insert
        // wins, a re-claim by the current holder is a no-op success. A
        // refused claim checks the incumbent's own pulse instead of
        // sweeping the whole table: a dead incumbent is evicted (the same
        // cascade age-out uses) and the claim retried once, so failover
        // stays instant without a DELETE on every claim. The full sweep
        // runs on its own timer.
        let mut claimed = sqlx::query(
            "INSERT INTO leases (object_id, node_id) VALUES ($1, $2)
             ON CONFLICT (object_id) DO NOTHING",
        )
        .bind(&request.object_id)
        .bind(node_id)
        .execute(&self.database)
        .await?;

        if claimed.rows_affected() == 0 {
            let stale: Option<Uuid> = sqlx::query_scalar(
                "SELECT l.node_id FROM leases l
                 LEFT JOIN nodes n ON n.id = l.node_id
                 WHERE l.object_id = $1
                   AND (n.id IS NULL OR n.last_heartbeat <= $2)",
            )
            .bind(&request.object_id)
            .bind(self.cutoff())
            .fetch_optional(&self.database)
            .await?;
            if let Some(dead) = stale {
                sqlx::query("DELETE FROM nodes WHERE id = $1")
                    .bind(dead)
                    .execute(&self.database)
                    .await?;
                // The cascade freed the lease unless the row was already
                // orphaned; clear it either way before retrying.
                sqlx::query("DELETE FROM leases WHERE object_id = $1 AND node_id = $2")
                    .bind(&request.object_id)
                    .bind(dead)
                    .execute(&self.database)
                    .await?;
                claimed = sqlx::query(
                    "INSERT INTO leases (object_id, node_id) VALUES ($1, $2)
                     ON CONFLICT (object_id) DO NOTHING",
                )
                .bind(&request.object_id)
                .bind(node_id)
                .execute(&self.database)
                .await?;
            }
        }

        // The claim carries its preimage; the directory keeps it so the
        // data stays enumerable after the declaring revision is gone.
        // Every claim restates the lifetime (touch refreshes, policy
        // changes apply on next touch, 0 clears) and backfills the hash
        // on rows from before the lifetime migration; the creator is
        // kept from the first claim only.
        if let Some((scope_id, script_id)) = identity {
            sqlx::query(
                "INSERT INTO object_instances
                     (scope_id, class, name, script_id, object_id,
                      created_by, expire_at)
                 VALUES ($1, $2, $3, $4, $5, NULLIF($6, ''),
                         CASE WHEN $7 > 0
                              THEN now() + make_interval(secs => $7)
                              ELSE NULL END)
                 ON CONFLICT (scope_id, class, name) DO UPDATE
                 SET object_id = COALESCE(object_instances.object_id,
                                          EXCLUDED.object_id),
                     expire_at = EXCLUDED.expire_at",
            )
            .bind(scope_id)
            .bind(&request.class)
            .bind(&request.name)
            .bind(script_id)
            .bind(&request.object_id)
            .bind(&request.created_by)
            .bind(request.expire_secs.min(i64::MAX as u64) as f64)
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
        let nodes = sqlx::query_as::<_, DbNode>(
            "SELECT * FROM nodes WHERE last_heartbeat > $1 ORDER BY registered",
        )
        .bind(self.cutoff())
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
        // treat it; the liveness filter does it without a delete, and
        // the sweep timer does the physical ageing.
        let holder: Option<Uuid> = sqlx::query_scalar(
            "SELECT l.node_id FROM leases l
             JOIN nodes n ON n.id = l.node_id AND n.last_heartbeat > $2
             WHERE l.object_id = $1",
        )
        .bind(&object_id)
        .bind(self.cutoff())
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

    async fn deregister(
        &self,
        request: Request<DeregisterRequest>,
    ) -> Result<Response<()>, Status> {
        let id = Uuid::from_str(&request.get_ref().node_id)
            .map_err(|_| RegistryError::InvalidId("node_id"))?;

        // A goodbye is age-out brought forward: the same deletion, the
        // same lease-freeing cascade, none of the waiting.
        sqlx::query("DELETE FROM nodes WHERE id = $1")
            .bind(id)
            .execute(&self.database)
            .await
            .map_err(RegistryError::Store)?;

        Ok(Response::new(()))
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

    async fn set_alarm(&self, request: Request<SetAlarmRequest>) -> Result<Response<()>, Status> {
        let request = request.get_ref();

        // One alarm per object, setting replaces: the same shape the
        // object's own persisted row has.
        sqlx::query(
            "INSERT INTO object_alarms (object_id, own_key, due_ms) VALUES ($1, $2, $3)
             ON CONFLICT (object_id) DO UPDATE SET own_key = $2, due_ms = $3",
        )
        .bind(&request.object_id)
        .bind(&request.own_key)
        .bind(request.due_ms)
        .execute(&self.database)
        .await
        .map_err(RegistryError::Store)?;

        Ok(Response::new(()))
    }

    async fn clear_alarm(
        &self,
        request: Request<ClearAlarmRequest>,
    ) -> Result<Response<()>, Status> {
        sqlx::query("DELETE FROM object_alarms WHERE object_id = $1")
            .bind(&request.get_ref().object_id)
            .execute(&self.database)
            .await
            .map_err(RegistryError::Store)?;

        Ok(Response::new(()))
    }

    async fn due_alarms(
        &self,
        request: Request<DueAlarmsRequest>,
    ) -> Result<Response<DueAlarmsResponse>, Status> {
        let request = request.get_ref();
        let limit = i64::from(request.limit.clamp(1, 1024));

        // Deliberately NOT filtered by holder liveness: a due alarm on a
        // dead node's object is exactly the row this query exists for.
        let rows: Vec<(String, String, i64)> = sqlx::query_as(
            "SELECT object_id, own_key, due_ms FROM object_alarms
             WHERE due_ms <= $1 ORDER BY due_ms LIMIT $2",
        )
        .bind(request.now_ms)
        .bind(limit)
        .fetch_all(&self.database)
        .await
        .map_err(RegistryError::Store)?;

        Ok(Response::new(DueAlarmsResponse {
            alarms: rows
                .into_iter()
                .map(|(object_id, own_key, due_ms)| AlarmRow {
                    object_id,
                    own_key,
                    due_ms,
                })
                .collect(),
        }))
    }

    async fn list_instances(
        &self,
        request: Request<ListInstancesRequest>,
    ) -> Result<Response<ListInstancesResponse>, Status> {
        let request = request.get_ref();
        let project_ids: Vec<Uuid> = request
            .project_ids
            .iter()
            .filter_map(|id| Uuid::from_str(id).ok())
            .collect();

        // Empty filters match everything, so one query shape serves the
        // full listing, the class browse and the type-ahead all alike.
        let class_filter = request.class.clone();
        let prefix = like_prefix(&request.name_prefix);
        let limit = page_size(request.page_size);
        let offset = i64::from(request.page) * limit;

        let total: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM object_instances
             WHERE scope_id = ANY($1)
               AND ($2 = '' OR class = $2)
               AND name LIKE $3",
        )
        .bind(&project_ids)
        .bind(&class_filter)
        .bind(&prefix)
        .fetch_one(&self.database)
        .await
        .map_err(RegistryError::Store)?;

        // Cron rows scope to their script, so a project listing never
        // matches them: only resource identities surface here. The
        // lifetime join rides the hash column; rows from before the
        // lifetime migration read as cold and alarmless, which they are.
        #[allow(clippy::type_complexity)]
        let rows: Vec<(
            Uuid,
            String,
            String,
            Uuid,
            i64,
            i64,
            i64,
            i64,
            String,
            String,
        )> = sqlx::query_as(
            "SELECT i.scope_id, i.class, i.name, i.script_id,
                        (EXTRACT(EPOCH FROM i.created) * 1000)::BIGINT,
                        COALESCE((EXTRACT(EPOCH FROM i.expire_at) * 1000)::BIGINT, 0),
                        COALESCE((EXTRACT(EPOCH FROM i.deleted_at) * 1000)::BIGINT, 0),
                        COALESCE(a.due_ms, 0),
                        COALESCE(l.node_id::text, ''),
                        COALESCE(i.created_by, '')
                 FROM object_instances i
                 LEFT JOIN object_alarms a ON a.object_id = i.object_id
                 LEFT JOIN leases l ON l.object_id = i.object_id
                 WHERE i.scope_id = ANY($1)
                   AND ($2 = '' OR i.class = $2)
                   AND i.name LIKE $3
                 ORDER BY i.class, i.name
                 LIMIT $4 OFFSET $5",
        )
        .bind(&project_ids)
        .bind(&class_filter)
        .bind(&prefix)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.database)
        .await
        .map_err(RegistryError::Store)?;

        Ok(Response::new(ListInstancesResponse {
            instances: rows
                .into_iter()
                .map(
                    |(
                        scope_id,
                        class,
                        name,
                        script_id,
                        created_ms,
                        expire_at_ms,
                        deleted_at_ms,
                        alarm_due_ms,
                        node_id,
                        created_by,
                    )| ObjectInstance {
                        scope_id: scope_id.to_string(),
                        class,
                        name,
                        script_id: script_id.to_string(),
                        created_ms,
                        expire_at_ms,
                        deleted_at_ms,
                        alarm_due_ms,
                        node_id,
                        created_by,
                    },
                )
                .collect(),
            total: total.max(0) as u64,
        }))
    }

    async fn count_instances(
        &self,
        request: Request<CountInstancesRequest>,
    ) -> Result<Response<CountInstancesResponse>, Status> {
        let project_ids: Vec<Uuid> = request
            .get_ref()
            .project_ids
            .iter()
            .filter_map(|id| Uuid::from_str(id).ok())
            .collect();

        let rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT class, count(*) FROM object_instances
             WHERE scope_id = ANY($1)
             GROUP BY class ORDER BY class",
        )
        .bind(&project_ids)
        .fetch_all(&self.database)
        .await
        .map_err(RegistryError::Store)?;

        Ok(Response::new(CountInstancesResponse {
            counts: rows
                .into_iter()
                .map(|(class, count)| ClassCount {
                    class,
                    count: count.max(0) as u64,
                })
                .collect(),
        }))
    }

    async fn delete_instance(
        &self,
        request: Request<DeleteInstanceRequest>,
    ) -> Result<Response<DeleteInstanceResponse>, Status> {
        let request = request.get_ref();
        let scope_id =
            Uuid::from_str(&request.scope_id).map_err(|_| RegistryError::InvalidId("scope_id"))?;

        // The commit point, one transaction: tombstone plus epoch bump.
        // The sweep's guard re-checks its predicate here, so a claim
        // that refreshed the row between query and tombstone wins.
        let mut tx = self.database.begin().await.map_err(RegistryError::Store)?;
        let tombstoned = sqlx::query(
            "UPDATE object_instances SET deleted_at = now()
             WHERE scope_id = $1 AND class = $2 AND name = $3
               AND deleted_at IS NULL
               AND ($4 = false
                    OR (expire_at IS NOT NULL AND expire_at <= now()
                        AND NOT EXISTS (SELECT 1 FROM object_alarms a
                                        WHERE a.object_id = $5)))",
        )
        .bind(scope_id)
        .bind(&request.class)
        .bind(&request.name)
        .bind(request.only_if_expired)
        .bind(&request.object_id)
        .execute(&mut *tx)
        .await
        .map_err(RegistryError::Store)?
        .rows_affected()
            == 1;

        if !tombstoned {
            tx.rollback().await.map_err(RegistryError::Store)?;
            return Ok(Response::new(DeleteInstanceResponse {
                tombstoned: false,
                epoch: 0,
            }));
        }

        // Everything after the tombstone happens under a newer epoch
        // than any pre-deletion holder ever held; the row itself is
        // kept forever so recreation keeps losing to nothing.
        let epoch: i64 = sqlx::query_scalar(
            "INSERT INTO object_epochs (object_id) VALUES ($1)
             ON CONFLICT (object_id)
             DO UPDATE SET epoch = object_epochs.epoch + 1
             RETURNING epoch",
        )
        .bind(&request.object_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(RegistryError::Store)?;
        tx.commit().await.map_err(RegistryError::Store)?;

        Ok(Response::new(DeleteInstanceResponse {
            tombstoned: true,
            epoch: epoch.max(1) as u64,
        }))
    }

    async fn purge_instance(
        &self,
        request: Request<PurgeInstanceRequest>,
    ) -> Result<Response<()>, Status> {
        let request = request.get_ref();
        let scope_id =
            Uuid::from_str(&request.scope_id).map_err(|_| RegistryError::InvalidId("scope_id"))?;

        // The end of the sequence, idempotent so the janitor can retry
        // it: the lease goes, then the directory row leaves the listing.
        // Alarms go too; a deleted object has no future obligations.
        sqlx::query("DELETE FROM leases WHERE object_id = $1")
            .bind(&request.object_id)
            .execute(&self.database)
            .await
            .map_err(RegistryError::Store)?;
        sqlx::query("DELETE FROM object_alarms WHERE object_id = $1")
            .bind(&request.object_id)
            .execute(&self.database)
            .await
            .map_err(RegistryError::Store)?;
        sqlx::query(
            "DELETE FROM object_instances
             WHERE scope_id = $1 AND class = $2 AND name = $3
               AND deleted_at IS NOT NULL",
        )
        .bind(scope_id)
        .bind(&request.class)
        .bind(&request.name)
        .execute(&self.database)
        .await
        .map_err(RegistryError::Store)?;

        Ok(Response::new(()))
    }

    async fn due_expiries(
        &self,
        request: Request<DueExpiriesRequest>,
    ) -> Result<Response<DueExpiriesResponse>, Status> {
        let request = request.get_ref();
        let limit = i64::from(request.limit.clamp(1, 1024));

        // Past due, not tombstoned, and not waiting: an alarm means the
        // instance has a future, and futures block expiry.
        let rows: Vec<(Uuid, String, String)> = sqlx::query_as(
            "SELECT i.scope_id, i.class, i.name
             FROM object_instances i
             LEFT JOIN object_alarms a ON a.object_id = i.object_id
             WHERE i.expire_at IS NOT NULL
               AND i.expire_at <= to_timestamp($1 / 1000.0)
               AND i.deleted_at IS NULL
               AND i.object_id IS NOT NULL
               AND a.object_id IS NULL
             ORDER BY i.expire_at
             LIMIT $2",
        )
        .bind(request.now_ms)
        .bind(limit)
        .fetch_all(&self.database)
        .await
        .map_err(RegistryError::Store)?;

        Ok(Response::new(DueExpiriesResponse {
            rows: rows
                .into_iter()
                .map(|(scope_id, class, name)| ExpiryRow {
                    scope_id: scope_id.to_string(),
                    class,
                    name,
                })
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

        // The listing filtered the dead node out; the reap timer's verb
        // makes the ageing physical, so the table never accumulates a
        // graveyard between ticks.
        registry.reap_expired().await.expect("the reap runs");
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
            ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
    async fn a_deregistered_node_frees_its_leases_immediately() {
        let (registry, _database, _guard) = registry(3600).await;

        let leaver = register(&registry, "leaver:3100").await;
        let heir = register(&registry, "heir:3100").await;
        let object = "e".repeat(64);

        registry
            .acquire_lease(Request::new(AcquireLeaseRequest {
                object_id: object.clone(),
                node_id: leaver.clone(),
                ..Default::default()
            }))
            .await
            .expect("claims");

        // The ttl is an hour; only the goodbye can free this today.
        registry
            .deregister(Request::new(DeregisterRequest {
                node_id: leaver.clone(),
            }))
            .await
            .expect("deregisters");

        let won = registry
            .acquire_lease(Request::new(AcquireLeaseRequest {
                object_id: object,
                node_id: heir.clone(),
                ..Default::default()
            }))
            .await
            .expect("claim answers")
            .into_inner();
        assert!(won.acquired, "the goodbye must free the lease at once");
        assert_eq!(won.node_id, heir);
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
    async fn a_seeded_class_of_ten_thousand_pages_by_prefix() {
        let (registry, database, _guard) = registry(60).await;
        let project = Uuid::new_v4();
        let script = Uuid::new_v4();

        // Seeded directly: 10k claims through the rpc would test postgres
        // insert speed, not the directory; the rows are the same.
        sqlx::query(
            "INSERT INTO object_instances (scope_id, class, name, script_id)
             SELECT $1, 'UserCart', 'user-' || lpad(n::text, 5, '0'), $2
             FROM generate_series(1, 10000) AS n",
        )
        .bind(project)
        .bind(script)
        .execute(&database)
        .await
        .expect("seeds");
        sqlx::query(
            "INSERT INTO object_instances (scope_id, class, name, script_id)
             VALUES ($1, 'Warehouse', 'eu-west', $2)",
        )
        .bind(project)
        .bind(script)
        .execute(&database)
        .await
        .expect("seeds the small class");

        let list = |class: &str, prefix: &str, page: u32, page_size: u32| {
            let registry = &registry;
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
            vec![("UserCart", 10_000), ("Warehouse", 1)],
        );

        // A prefix narrows 10k to the ten `user-0042x` names, paged.
        let narrowed = list("UserCart", "user-0042", 0, 4).await;
        assert_eq!(narrowed.total, 10, "user-00420..user-00429");
        assert_eq!(narrowed.instances.len(), 4);
        assert_eq!(narrowed.instances[0].name, "user-00420");
        let last_page = list("UserCart", "user-0042", 2, 4).await;
        assert_eq!(last_page.instances.len(), 2, "10 rows = pages of 4,4,2");
        assert_eq!(last_page.instances[1].name, "user-00429");

        // A LIKE wildcard in the prefix is a literal, not a wildcard.
        let literal = list("UserCart", "user-%", 0, 10).await;
        assert_eq!(literal.total, 0, "nobody is named 'user-%...'");

        // The unfiltered listing clamps instead of returning 10k rows.
        let unfiltered = list("", "", 0, 0).await;
        assert_eq!(unfiltered.total, 10_001);
        assert_eq!(unfiltered.instances.len(), 100, "the default page");
    }

    #[tokio::test]
    async fn a_due_alarm_outlives_its_holder_and_answers_the_sweep() {
        let (registry, database, _guard) = registry(45).await;

        let holder = register(&registry, "holder:3100").await;
        let object = "d".repeat(64);

        registry
            .acquire_lease(Request::new(AcquireLeaseRequest {
                object_id: object.clone(),
                node_id: holder.clone(),
                ..Default::default()
            }))
            .await
            .expect("claims");

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

        // The holder dies. Its lease frees through the cascade; the alarm
        // row must NOT: firing it is now some survivor's job.
        backdate(&database, &holder, 46).await;
        let due = registry
            .due_alarms(Request::new(DueAlarmsRequest {
                now_ms: 2_000,
                limit: 10,
            }))
            .await
            .expect("the sweep queries")
            .into_inner()
            .alarms;
        assert_eq!(due.len(), 1, "one row per object, latest write");
        assert_eq!(due[0].own_key, "proj-1/Keeper/watchdog");
        assert_eq!(due[0].due_ms, 2_000);

        // A future alarm is not due yet.
        let early = registry
            .due_alarms(Request::new(DueAlarmsRequest {
                now_ms: 1_999,
                limit: 10,
            }))
            .await
            .expect("queries")
            .into_inner()
            .alarms;
        assert!(early.is_empty(), "not due yet: {early:?}");

        // Fired (or cleared): the row goes, the sweep goes quiet.
        registry
            .clear_alarm(Request::new(ClearAlarmRequest {
                object_id: object.clone(),
            }))
            .await
            .expect("clears");
        let after = registry
            .due_alarms(Request::new(DueAlarmsRequest {
                now_ms: i64::MAX,
                limit: 10,
            }))
            .await
            .expect("queries")
            .into_inner()
            .alarms;
        assert!(after.is_empty());
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

    #[tokio::test]
    async fn the_lifecycle_round_trips_through_the_directory() {
        let (registry, _database, _guard) = registry(45).await;
        let node = register(&registry, "holder:3000").await;
        let project = Uuid::new_v4();
        let script = Uuid::new_v4();

        // A claim with a lifespan stamps expiry and the hash.
        let lease = registry
            .acquire_lease(Request::new(AcquireLeaseRequest {
                object_id: "hash-doomed".to_owned(),
                node_id: node.clone(),
                scope_id: project.to_string(),
                class: "Session".to_owned(),
                name: "doomed".to_owned(),
                script_id: script.to_string(),
                expire_secs: 1,
                created_by: String::new(),
            }))
            .await
            .expect("claim answers")
            .into_inner();
        assert!(lease.acquired);
        let first_epoch = lease.epoch;

        // Not due yet; then past due, one row; an alarm blocks it again.
        let now = || {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_millis() as i64
        };
        let due = |at: i64| DueExpiriesRequest {
            now_ms: at,
            limit: 16,
        };
        let before = registry
            .due_expiries(Request::new(due(now())))
            .await
            .expect("due answers")
            .into_inner();
        assert!(before.rows.is_empty(), "not due yet: {before:?}");

        let later = now() + 2_000;
        let past = registry
            .due_expiries(Request::new(due(later)))
            .await
            .expect("due answers")
            .into_inner();
        assert_eq!(past.rows.len(), 1);
        assert_eq!(past.rows[0].name, "doomed");

        registry
            .set_alarm(Request::new(SetAlarmRequest {
                object_id: "hash-doomed".to_owned(),
                own_key: "Session/doomed".to_owned(),
                due_ms: later + 60_000,
            }))
            .await
            .expect("alarm sets");
        let blocked = registry
            .due_expiries(Request::new(due(later)))
            .await
            .expect("due answers")
            .into_inner();
        assert!(blocked.rows.is_empty(), "an alarm blocks expiry");

        // The sweep's guarded tombstone refuses while the alarm stands;
        // an external (unguarded) delete does not.
        let refused = registry
            .delete_instance(Request::new(DeleteInstanceRequest {
                scope_id: project.to_string(),
                class: "Session".to_owned(),
                name: "doomed".to_owned(),
                object_id: "hash-doomed".to_owned(),
                only_if_expired: true,
            }))
            .await
            .expect("delete answers")
            .into_inner();
        assert!(!refused.tombstoned);

        let deleted = registry
            .delete_instance(Request::new(DeleteInstanceRequest {
                scope_id: project.to_string(),
                class: "Session".to_owned(),
                name: "doomed".to_owned(),
                object_id: "hash-doomed".to_owned(),
                only_if_expired: false,
            }))
            .await
            .expect("delete answers")
            .into_inner();
        assert!(deleted.tombstoned);
        assert!(deleted.epoch > first_epoch, "the tombstone bumps the fence");

        // Tombstoned: claims refuse, the listing shows the state, and a
        // second tombstone is a no-op.
        let refused_claim = registry
            .acquire_lease(Request::new(AcquireLeaseRequest {
                object_id: "hash-doomed".to_owned(),
                node_id: node.clone(),
                scope_id: project.to_string(),
                class: "Session".to_owned(),
                name: "doomed".to_owned(),
                script_id: script.to_string(),
                ..Default::default()
            }))
            .await;
        assert!(refused_claim.is_err(), "a deleting identity is not a home");

        let listed = registry
            .list_instances(Request::new(ListInstancesRequest {
                project_ids: vec![project.to_string()],
                ..Default::default()
            }))
            .await
            .expect("list answers")
            .into_inner();
        assert_eq!(listed.instances.len(), 1);
        assert!(listed.instances[0].deleted_at_ms > 0);

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

        let recreated = registry
            .acquire_lease(Request::new(AcquireLeaseRequest {
                object_id: "hash-doomed".to_owned(),
                node_id: node,
                scope_id: project.to_string(),
                class: "Session".to_owned(),
                name: "doomed".to_owned(),
                script_id: script.to_string(),
                ..Default::default()
            }))
            .await
            .expect("recreation claims")
            .into_inner();
        assert!(recreated.acquired);
        assert!(
            recreated.epoch > deleted.epoch,
            "a fresh life starts above every old fence"
        );
    }
}
