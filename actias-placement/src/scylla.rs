//! The scylla backend, the fleet's: every operation reads one partition,
//! a claim is one lightweight transaction, and epochs come from the
//! identity's own memory rather than a sequence, since nothing is global.
//!
//! What postgres did with a cascade and a transaction is done in the
//! open here: a lease is written twice (by object and by holder), a
//! departure is captured from the holder's list before its leases go,
//! and time-ordered sweeps read per-minute buckets whose non-empty set
//! is kept in a bucket table so a quiet sweep costs one read.

use std::collections::BTreeSet;

use scylla::client::session::Session;
use scylla::errors::ExecutionError;
use scylla::statement::prepared::PreparedStatement;
use scylla::statement::{Consistency, SerialConsistency};
use uuid::Uuid;

use crate::proto_node_registry::{
    AcquireLeaseRequest, AlarmRow, ClassCount, DeletionRow, ExpiryRow, Lease, ObjectInstance,
};
use crate::store::{
    Departed, HeldIdentity, Identity, NodeRow, PlacementStore, RegistryError, next_epoch, now_ms,
};

impl From<ExecutionError> for RegistryError {
    fn from(error: ExecutionError) -> Self {
        RegistryError::Store(error.to_string())
    }
}

/// Collapses the driver's per-stage result errors into a store error;
/// they all mean the same thing to a caller, a result of the wrong shape.
fn rows_error<E: std::fmt::Display>(error: E) -> RegistryError {
    RegistryError::Store(error.to_string())
}

/// Milliseconds per sweep bucket.
const BUCKET_MS: i64 = 60_000;

fn bucket(ms: i64) -> i64 {
    ms.div_euclid(BUCKET_MS)
}

/// The end of a name range that starts with `prefix`.
fn prefix_end(prefix: &str) -> String {
    format!("{prefix}\u{10FFFF}")
}

/// The one row shape every instance read produces.
type InstanceTuple = (
    String,
    Option<String>,
    Uuid,
    Option<i64>,
    Option<String>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
);

struct Statements {
    register: PreparedStatement,
    heartbeat: PreparedStatement,
    nodes: PreparedStatement,
    node: PreparedStatement,
    delete_node: PreparedStatement,
    claim: PreparedStatement,
    lease: PreparedStatement,
    move_row: PreparedStatement,
    set_move: PreparedStatement,
    clear_move: PreparedStatement,
    lease_by_node: PreparedStatement,
    leases_of: PreparedStatement,
    delete_lease_if: PreparedStatement,
    raise_lease: PreparedStatement,
    delete_lease: PreparedStatement,
    delete_lease_by_node: PreparedStatement,
    delete_leases_by_node: PreparedStatement,
    instance: PreparedStatement,
    instance_insert: PreparedStatement,
    instance_touch: PreparedStatement,
    instance_tombstone: PreparedStatement,
    instance_epoch: PreparedStatement,
    instance_delete: PreparedStatement,
    instances_range: PreparedStatement,
    instances_all: PreparedStatement,
    class_insert: PreparedStatement,
    classes: PreparedStatement,
    instance_id_insert: PreparedStatement,
    instance_id: PreparedStatement,
    instance_id_delete: PreparedStatement,
    alarm_insert: PreparedStatement,
    alarm_delete: PreparedStatement,
    alarm_by_object_insert: PreparedStatement,
    alarm_by_object: PreparedStatement,
    alarm_by_object_delete: PreparedStatement,
    alarm_bucket_insert: PreparedStatement,
    alarm_buckets: PreparedStatement,
    alarm_bucket_delete: PreparedStatement,
    alarms_due: PreparedStatement,
    expiry_insert: PreparedStatement,
    expiry_delete: PreparedStatement,
    expiry_bucket_insert: PreparedStatement,
    expiry_buckets: PreparedStatement,
    expiry_bucket_delete: PreparedStatement,
    expiries_due: PreparedStatement,
    tombstone_insert: PreparedStatement,
    tombstone_delete: PreparedStatement,
    tombstones: PreparedStatement,
    departure_insert: PreparedStatement,
    departures: PreparedStatement,
    departure_take: PreparedStatement,
}

pub struct ScyllaStore {
    session: Session,
    region: String,
    statements: Statements,
}

/// Connects a session pointed at the placement keyspace.
///
/// # Panics
/// Panics when no node accepts a session; this runs at startup, where
/// dying loudly is the right outcome.
pub async fn connect(scylla_nodes: Vec<String>) -> Session {
    scylla::client::session_builder::SessionBuilder::new()
        .known_nodes(scylla_nodes)
        .use_keyspace("placement", true)
        .build()
        .await
        .expect("scylla session could not be established from SCYLLA_NODES")
}

impl ScyllaStore {
    pub async fn new(session: Session, region: String) -> Self {
        let prepare = |cql: &'static str, serial: bool| {
            let session = &session;
            async move {
                let mut statement = session
                    .prepare(cql)
                    .await
                    .unwrap_or_else(|e| panic!("failed to prepare {cql:?}: {e}"));
                statement.set_consistency(Consistency::LocalQuorum);
                if serial {
                    statement.set_serial_consistency(Some(SerialConsistency::LocalSerial));
                }
                statement
            }
        };
        let statements = Statements {
            register: prepare(
                "INSERT INTO nodes (region, node_id, address, capabilities, load, registered_ms, last_heartbeat_ms) VALUES (?, ?, ?, ?, 0, ?, ?)",
                false,
            )
            .await,
            heartbeat: prepare(
                "UPDATE nodes SET last_heartbeat_ms = ?, load = ? WHERE region = ? AND node_id = ? IF last_heartbeat_ms > ?",
                true,
            )
            .await,
            nodes: prepare(
                "SELECT node_id, address, capabilities, load, registered_ms, last_heartbeat_ms FROM nodes WHERE region = ?",
                false,
            )
            .await,
            node: prepare(
                "SELECT node_id, address, capabilities, load, registered_ms, last_heartbeat_ms FROM nodes WHERE region = ? AND node_id = ?",
                false,
            )
            .await,
            delete_node: prepare("DELETE FROM nodes WHERE region = ? AND node_id = ?", false).await,
            claim: prepare(
                "INSERT INTO leases (object_id, node_id, epoch, acquired_ms) VALUES (?, ?, ?, ?) IF NOT EXISTS",
                true,
            )
            .await,
            lease: prepare("SELECT node_id, epoch FROM leases WHERE object_id = ?", false).await,
            move_row: prepare("SELECT region FROM moves WHERE object_id = ?", false).await,
            set_move: prepare(
                "INSERT INTO moves (object_id, region, moved_ms) VALUES (?, ?, ?)",
                false,
            )
            .await,
            clear_move: prepare("DELETE FROM moves WHERE object_id = ?", false).await,
            lease_by_node: prepare(
                "INSERT INTO leases_by_node (node_id, object_id) VALUES (?, ?)",
                false,
            )
            .await,
            leases_of: prepare("SELECT object_id FROM leases_by_node WHERE node_id = ?", false).await,
            delete_lease_if: prepare("DELETE FROM leases WHERE object_id = ? IF node_id = ?", true).await,
            raise_lease: prepare(
                "UPDATE leases SET epoch = ? WHERE object_id = ? IF node_id = ? AND epoch < ?",
                true,
            )
            .await,
            delete_lease: prepare("DELETE FROM leases WHERE object_id = ?", false).await,
            delete_lease_by_node: prepare(
                "DELETE FROM leases_by_node WHERE node_id = ? AND object_id = ?",
                false,
            )
            .await,
            delete_leases_by_node: prepare("DELETE FROM leases_by_node WHERE node_id = ?", false).await,
            instance: prepare(
                "SELECT name, object_id, script_id, created_ms, created_by, expire_at_ms, deleted_at_ms, last_epoch FROM instances WHERE scope_id = ? AND class = ? AND name = ?",
                false,
            )
            .await,
            instance_insert: prepare(
                "INSERT INTO instances (scope_id, class, name, object_id, script_id, created_ms, created_by, expire_at_ms, last_epoch) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) IF NOT EXISTS",
                true,
            )
            .await,
            instance_touch: prepare(
                "UPDATE instances SET object_id = ?, expire_at_ms = ?, last_epoch = ? WHERE scope_id = ? AND class = ? AND name = ?",
                false,
            )
            .await,
            instance_tombstone: prepare(
                "UPDATE instances SET deleted_at_ms = ?, last_epoch = ? WHERE scope_id = ? AND class = ? AND name = ? IF deleted_at_ms = null",
                true,
            )
            .await,
            instance_epoch: prepare(
                "UPDATE instances SET last_epoch = ? WHERE scope_id = ? AND class = ? AND name = ?",
                false,
            )
            .await,
            instance_delete: prepare(
                "DELETE FROM instances WHERE scope_id = ? AND class = ? AND name = ?",
                false,
            )
            .await,
            instances_range: prepare(
                "SELECT name, object_id, script_id, created_ms, created_by, expire_at_ms, deleted_at_ms, last_epoch FROM instances WHERE scope_id = ? AND class = ? AND name >= ? AND name < ?",
                false,
            )
            .await,
            instances_all: prepare(
                "SELECT name, object_id, script_id, created_ms, created_by, expire_at_ms, deleted_at_ms, last_epoch FROM instances WHERE scope_id = ? AND class = ?",
                false,
            )
            .await,
            class_insert: prepare("INSERT INTO classes (scope_id, class) VALUES (?, ?)", false).await,
            classes: prepare("SELECT class FROM classes WHERE scope_id = ?", false).await,
            instance_id_insert: prepare(
                "INSERT INTO instance_ids (object_id, scope_id, class, name) VALUES (?, ?, ?, ?)",
                false,
            )
            .await,
            instance_id: prepare(
                "SELECT scope_id, class, name FROM instance_ids WHERE object_id = ?",
                false,
            )
            .await,
            instance_id_delete: prepare("DELETE FROM instance_ids WHERE object_id = ?", false).await,
            alarm_insert: prepare(
                "INSERT INTO alarms (region, due_bucket, due_ms, object_id, own_key) VALUES (?, ?, ?, ?, ?)",
                false,
            )
            .await,
            alarm_delete: prepare(
                "DELETE FROM alarms WHERE region = ? AND due_bucket = ? AND due_ms = ? AND object_id = ?",
                false,
            )
            .await,
            alarm_by_object_insert: prepare(
                "INSERT INTO alarms_by_object (object_id, due_ms, own_key) VALUES (?, ?, ?)",
                false,
            )
            .await,
            alarm_by_object: prepare(
                "SELECT due_ms, own_key FROM alarms_by_object WHERE object_id = ?",
                false,
            )
            .await,
            alarm_by_object_delete: prepare("DELETE FROM alarms_by_object WHERE object_id = ?", false).await,
            alarm_bucket_insert: prepare(
                "INSERT INTO alarm_buckets (region, due_bucket) VALUES (?, ?)",
                false,
            )
            .await,
            alarm_buckets: prepare("SELECT due_bucket FROM alarm_buckets WHERE region = ?", false).await,
            alarm_bucket_delete: prepare(
                "DELETE FROM alarm_buckets WHERE region = ? AND due_bucket = ?",
                false,
            )
            .await,
            alarms_due: prepare(
                "SELECT due_ms, object_id, own_key FROM alarms WHERE region = ? AND due_bucket = ? AND due_ms <= ?",
                false,
            )
            .await,
            expiry_insert: prepare(
                "INSERT INTO expiries (region, due_bucket, expire_at_ms, scope_id, class, name) VALUES (?, ?, ?, ?, ?, ?)",
                false,
            )
            .await,
            expiry_delete: prepare(
                "DELETE FROM expiries WHERE region = ? AND due_bucket = ? AND expire_at_ms = ? AND scope_id = ? AND class = ? AND name = ?",
                false,
            )
            .await,
            expiry_bucket_insert: prepare(
                "INSERT INTO expiry_buckets (region, due_bucket) VALUES (?, ?)",
                false,
            )
            .await,
            expiry_buckets: prepare("SELECT due_bucket FROM expiry_buckets WHERE region = ?", false).await,
            expiry_bucket_delete: prepare(
                "DELETE FROM expiry_buckets WHERE region = ? AND due_bucket = ?",
                false,
            )
            .await,
            expiries_due: prepare(
                "SELECT expire_at_ms, scope_id, class, name FROM expiries WHERE region = ? AND due_bucket = ? AND expire_at_ms <= ?",
                false,
            )
            .await,
            tombstone_insert: prepare(
                "INSERT INTO tombstones (region, deleted_at_ms, scope_id, class, name) VALUES (?, ?, ?, ?, ?)",
                false,
            )
            .await,
            tombstone_delete: prepare(
                "DELETE FROM tombstones WHERE region = ? AND deleted_at_ms = ? AND scope_id = ? AND class = ? AND name = ?",
                false,
            )
            .await,
            tombstones: prepare(
                "SELECT deleted_at_ms, scope_id, class, name FROM tombstones WHERE region = ? AND deleted_at_ms <= ?",
                false,
            )
            .await,
            departure_insert: prepare(
                "INSERT INTO departures (region, node_id, drained, departed_ms, object_ids) VALUES (?, ?, ?, ?, ?) IF NOT EXISTS",
                true,
            )
            .await,
            departures: prepare(
                "SELECT node_id, drained, object_ids FROM departures WHERE region = ?",
                false,
            )
            .await,
            departure_take: prepare(
                "DELETE FROM departures WHERE region = ? AND node_id = ? IF EXISTS",
                true,
            )
            .await,
        };
        Self {
            session,
            region,
            statements,
        }
    }

    /// Whether a lightweight transaction applied: the first column of
    /// its result row.
    fn applied(result: scylla::response::query_result::QueryResult) -> Result<bool, RegistryError> {
        let rows = result.into_rows_result().map_err(rows_error)?;
        let row = rows
            .maybe_first_row::<scylla::value::Row>()
            .map_err(rows_error)?;
        Ok(row
            .and_then(|row| row.columns.into_iter().next().flatten())
            .and_then(|value| value.as_boolean())
            .unwrap_or(false))
    }

    async fn node_row(&self, id: Uuid) -> Result<Option<NodeRow>, RegistryError> {
        let rows = self
            .session
            .execute_unpaged(&self.statements.node, (&self.region, id))
            .await?
            .into_rows_result()
            .map_err(rows_error)?;
        Ok(rows
            .maybe_first_row::<(Uuid, String, Vec<String>, i32, i64, i64)>()
            .map_err(rows_error)?
            .map(
                |(id, address, capabilities, load, registered_ms, last_heartbeat_ms)| NodeRow {
                    id,
                    address,
                    capabilities,
                    load,
                    registered_ms,
                    last_heartbeat_ms,
                },
            ))
    }

    async fn lease_row(&self, object_id: &str) -> Result<Option<(Uuid, i64)>, RegistryError> {
        let rows = self
            .session
            .execute_unpaged(&self.statements.lease, (object_id,))
            .await?
            .into_rows_result()
            .map_err(rows_error)?;
        rows.maybe_first_row::<(Uuid, i64)>().map_err(rows_error)
    }

    async fn instance_row(
        &self,
        scope: Uuid,
        class: &str,
        name: &str,
    ) -> Result<Option<InstanceTuple>, RegistryError> {
        let rows = self
            .session
            .execute_unpaged(&self.statements.instance, (scope, class, name))
            .await?
            .into_rows_result()
            .map_err(rows_error)?;
        rows.maybe_first_row::<InstanceTuple>().map_err(rows_error)
    }

    async fn alarm_of(&self, object_id: &str) -> Result<Option<(i64, String)>, RegistryError> {
        let rows = self
            .session
            .execute_unpaged(&self.statements.alarm_by_object, (object_id,))
            .await?
            .into_rows_result()
            .map_err(rows_error)?;
        rows.maybe_first_row::<(i64, String)>().map_err(rows_error)
    }

    /// The objects a node holds, by its own list.
    async fn held_by(&self, node: Uuid) -> Result<Vec<String>, RegistryError> {
        let rows = self
            .session
            .execute_unpaged(&self.statements.leases_of, (node,))
            .await?
            .into_rows_result()
            .map_err(rows_error)?;
        rows.rows::<(String,)>()
            .map_err(rows_error)?
            .map(|row| row.map(|(id,)| id))
            .collect::<Result<Vec<_>, _>>()
            .map_err(rows_error)
    }

    /// The buckets a sweep table holds, in order.
    async fn buckets(&self, statement: &PreparedStatement) -> Result<Vec<i64>, RegistryError> {
        let rows = self
            .session
            .execute_unpaged(statement, (&self.region,))
            .await?
            .into_rows_result()
            .map_err(rows_error)?;
        let mut buckets: Vec<i64> = rows
            .rows::<(i64,)>()
            .map_err(rows_error)?
            .map(|row| row.map(|(bucket,)| bucket))
            .collect::<Result<Vec<_>, _>>()
            .map_err(rows_error)?;
        buckets.sort_unstable();
        Ok(buckets)
    }

    /// Records what `node` held and then frees it: the departure first,
    /// so the record exists before any lease is gone.
    async fn depart(&self, node: Uuid, drained: bool) -> Result<(), RegistryError> {
        let held = self.held_by(node).await?;
        self.session
            .execute_unpaged(
                &self.statements.departure_insert,
                (&self.region, node, drained, now_ms(), &held),
            )
            .await?;
        self.session
            .execute_unpaged(&self.statements.delete_node, (&self.region, node))
            .await?;
        for object_id in &held {
            self.session
                .execute_unpaged(&self.statements.delete_lease_if, (object_id, node))
                .await?;
        }
        self.session
            .execute_unpaged(&self.statements.delete_leases_by_node, (node,))
            .await?;
        Ok(())
    }

    /// Whether a node is alive at the cutoff; a missing row is dead.
    async fn alive(&self, node: Uuid, cutoff_ms: i64) -> Result<bool, RegistryError> {
        Ok(self
            .node_row(node)
            .await?
            .is_some_and(|row| row.last_heartbeat_ms > cutoff_ms))
    }

    /// The listing's row, with what the alarm and lease tables add.
    async fn describe(
        &self,
        scope: Uuid,
        class: &str,
        row: InstanceTuple,
    ) -> Result<ObjectInstance, RegistryError> {
        let (name, object_id, script_id, created_ms, created_by, expire_at_ms, deleted_at_ms, _) =
            row;
        let (alarm_due_ms, node_id) = match object_id.as_deref() {
            Some(id) => (
                self.alarm_of(id).await?.map(|(due, _)| due).unwrap_or(0),
                self.lease_row(id)
                    .await?
                    .map(|(node, _)| node.to_string())
                    .unwrap_or_default(),
            ),
            None => (0, String::new()),
        };
        Ok(ObjectInstance {
            scope_id: scope.to_string(),
            class: class.to_owned(),
            name,
            script_id: script_id.to_string(),
            created_ms: created_ms.unwrap_or(0),
            expire_at_ms: expire_at_ms.unwrap_or(0),
            deleted_at_ms: deleted_at_ms.unwrap_or(0),
            alarm_due_ms,
            node_id,
            created_by: created_by.unwrap_or_default(),
            object_id: object_id.unwrap_or_default(),
        })
    }

    async fn drop_expiry(
        &self,
        expire_at_ms: i64,
        scope: Uuid,
        class: &str,
        name: &str,
    ) -> Result<(), RegistryError> {
        self.session
            .execute_unpaged(
                &self.statements.expiry_delete,
                (
                    &self.region,
                    bucket(expire_at_ms),
                    expire_at_ms,
                    scope,
                    class,
                    name,
                ),
            )
            .await?;
        Ok(())
    }

    async fn drop_alarm(&self, object_id: &str) -> Result<(), RegistryError> {
        if let Some((due_ms, _)) = self.alarm_of(object_id).await? {
            self.session
                .execute_unpaged(
                    &self.statements.alarm_delete,
                    (&self.region, bucket(due_ms), due_ms, object_id),
                )
                .await?;
            self.session
                .execute_unpaged(&self.statements.alarm_by_object_delete, (object_id,))
                .await?;
        }
        Ok(())
    }

    /// Frees a lease whoever holds it, both tables.
    async fn free_lease(&self, object_id: &str) -> Result<(), RegistryError> {
        if let Some((node, _)) = self.lease_row(object_id).await? {
            self.session
                .execute_unpaged(&self.statements.delete_lease_by_node, (node, object_id))
                .await?;
        }
        self.session
            .execute_unpaged(&self.statements.delete_lease, (object_id,))
            .await?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl PlacementStore for ScyllaStore {
    async fn register(
        &self,
        address: &str,
        capabilities: &[String],
    ) -> Result<Uuid, RegistryError> {
        let id = Uuid::new_v4();
        let now = now_ms();
        self.session
            .execute_unpaged(
                &self.statements.register,
                (&self.region, id, address, capabilities, now, now),
            )
            .await?;
        Ok(id)
    }

    async fn heartbeat(
        &self,
        node: Uuid,
        load: i32,
        cutoff_ms: i64,
    ) -> Result<bool, RegistryError> {
        // Conditional on the row still being inside the ttl, so an
        // aged-out node is refused rather than resurrected.
        let result = self
            .session
            .execute_unpaged(
                &self.statements.heartbeat,
                (now_ms(), load, &self.region, node, cutoff_ms),
            )
            .await?;
        Self::applied(result)
    }

    async fn live_nodes(&self, cutoff_ms: i64) -> Result<Vec<NodeRow>, RegistryError> {
        let rows = self
            .session
            .execute_unpaged(&self.statements.nodes, (&self.region,))
            .await?
            .into_rows_result()
            .map_err(rows_error)?;
        let mut nodes: Vec<NodeRow> = rows
            .rows::<(Uuid, String, Vec<String>, i32, i64, i64)>()
            .map_err(rows_error)?
            .map(|row| {
                row.map(
                    |(id, address, capabilities, load, registered_ms, last_heartbeat_ms)| NodeRow {
                        id,
                        address,
                        capabilities,
                        load,
                        registered_ms,
                        last_heartbeat_ms,
                    },
                )
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(rows_error)?;
        nodes.retain(|node| node.last_heartbeat_ms > cutoff_ms);
        nodes.sort_by_key(|node| node.registered_ms);
        Ok(nodes)
    }

    async fn node(&self, id: Uuid, cutoff_ms: i64) -> Result<Option<NodeRow>, RegistryError> {
        Ok(self
            .node_row(id)
            .await?
            .filter(|node| node.last_heartbeat_ms > cutoff_ms))
    }

    async fn claim(
        &self,
        request: &AcquireLeaseRequest,
        node: Uuid,
        identity: Option<Identity>,
        cutoff_ms: i64,
    ) -> Result<Lease, RegistryError> {
        let object_id = request.object_id.as_str();

        // A tombstoned identity refuses claims until the janitor
        // finishes; the row also carries the identity's last epoch and
        // its current expiry, both needed below.
        let mut last_epoch = 0;
        let mut existing: Option<InstanceTuple> = None;
        if let Some((scope, _)) = identity {
            existing = self
                .instance_row(scope, &request.class, &request.name)
                .await?;
            if let Some(row) = &existing {
                if row.6.is_some() {
                    return Err(RegistryError::Deleting);
                }
                last_epoch = row.7.unwrap_or(0);
            }
        }

        // A forwarding row wins over any claim: the object was born here
        // and lives elsewhere now; the caller learns where and forwards.
        if let Some(region) = self.get_move(object_id).await? {
            return Ok(Lease {
                object_id: object_id.to_owned(),
                node_id: String::new(),
                acquired: false,
                epoch: 0,
                fresh: false,
                moved_to: region,
            });
        }

        // The conditional claim: first insert wins. A refused claim
        // checks the incumbent's own pulse; a dead incumbent is evicted
        // (departure recorded first) and the claim retried once.
        let mut epoch = next_epoch(last_epoch);
        let mut won = Self::applied(
            self.session
                .execute_unpaged(&self.statements.claim, (object_id, node, epoch, now_ms()))
                .await?,
        )?;
        if !won {
            let Some((incumbent, held_epoch)) = self.lease_row(object_id).await? else {
                return Err(RegistryError::ClaimRaced);
            };
            if incumbent != node && !self.alive(incumbent, cutoff_ms).await? {
                self.depart(incumbent, false).await?;
                epoch = next_epoch(last_epoch.max(held_epoch));
                won = Self::applied(
                    self.session
                        .execute_unpaged(&self.statements.claim, (object_id, node, epoch, now_ms()))
                        .await?,
                )?;
            }
        }
        if won {
            self.session
                .execute_unpaged(&self.statements.lease_by_node, (node, object_id))
                .await?;
        }

        // The lease row answers who holds it and under which epoch.
        let (holder, held_epoch) = self
            .lease_row(object_id)
            .await?
            .ok_or(RegistryError::ClaimRaced)?;
        let acquired = won || holder == node;

        // The claim carries its preimage; the directory keeps it. Every
        // claim restates the lifetime; the creator is kept from the
        // first claim only.
        let mut fresh = false;
        if let Some((scope, script)) = identity {
            let now = now_ms();
            let expire_at_ms = (request.expire_secs > 0)
                .then(|| now + request.expire_secs.min(i64::MAX as u64 / 2000) as i64 * 1000);
            match existing {
                None => {
                    let created_by =
                        (!request.created_by.is_empty()).then(|| request.created_by.clone());
                    fresh = Self::applied(
                        self.session
                            .execute_unpaged(
                                &self.statements.instance_insert,
                                (
                                    scope,
                                    &request.class,
                                    &request.name,
                                    object_id,
                                    script,
                                    now,
                                    created_by,
                                    expire_at_ms,
                                    held_epoch,
                                ),
                            )
                            .await?,
                    )?;
                    if !fresh {
                        // Raced another first claim; restate like a touch.
                        self.session
                            .execute_unpaged(
                                &self.statements.instance_touch,
                                (
                                    object_id,
                                    expire_at_ms,
                                    held_epoch,
                                    scope,
                                    &request.class,
                                    &request.name,
                                ),
                            )
                            .await?;
                    }
                    self.session
                        .execute_unpaged(&self.statements.class_insert, (scope, &request.class))
                        .await?;
                }
                Some(row) => {
                    let kept_id = row.1.clone().unwrap_or_else(|| object_id.to_owned());
                    if let Some(old) = row.5
                        && Some(old) != expire_at_ms
                    {
                        self.drop_expiry(old, scope, &request.class, &request.name)
                            .await?;
                    }
                    self.session
                        .execute_unpaged(
                            &self.statements.instance_touch,
                            (
                                kept_id,
                                expire_at_ms,
                                held_epoch.max(last_epoch),
                                scope,
                                &request.class,
                                &request.name,
                            ),
                        )
                        .await?;
                }
            }
            self.session
                .execute_unpaged(
                    &self.statements.instance_id_insert,
                    (object_id, scope, &request.class, &request.name),
                )
                .await?;
            if let Some(due) = expire_at_ms {
                self.session
                    .execute_unpaged(
                        &self.statements.expiry_insert,
                        (
                            &self.region,
                            bucket(due),
                            due,
                            scope,
                            &request.class,
                            &request.name,
                        ),
                    )
                    .await?;
                self.session
                    .execute_unpaged(
                        &self.statements.expiry_bucket_insert,
                        (&self.region, bucket(due)),
                    )
                    .await?;
            }
        }

        Ok(Lease {
            object_id: object_id.to_owned(),
            node_id: holder.to_string(),
            acquired,
            epoch: held_epoch.max(1) as u64,
            fresh,
            moved_to: String::new(),
        })
    }

    async fn set_move(&self, object_id: &str, region: &str) -> Result<(), RegistryError> {
        self.session
            .execute_unpaged(&self.statements.set_move, (object_id, region, now_ms()))
            .await?;
        Ok(())
    }

    async fn get_move(&self, object_id: &str) -> Result<Option<String>, RegistryError> {
        let rows = self
            .session
            .execute_unpaged(&self.statements.move_row, (object_id,))
            .await?
            .into_rows_result()
            .map_err(rows_error)?;
        Ok(rows
            .maybe_first_row::<(String,)>()
            .map_err(rows_error)?
            .map(|(region,)| region))
    }

    async fn clear_move(&self, object_id: &str) -> Result<(), RegistryError> {
        self.session
            .execute_unpaged(&self.statements.clear_move, (object_id,))
            .await?;
        Ok(())
    }

    async fn holder(
        &self,
        object_id: &str,
        cutoff_ms: i64,
    ) -> Result<Option<(Uuid, u64)>, RegistryError> {
        let Some((node, epoch)) = self.lease_row(object_id).await? else {
            return Ok(None);
        };
        if !self.alive(node, cutoff_ms).await? {
            return Ok(None);
        }
        Ok(Some((node, epoch.max(1) as u64)))
    }

    async fn raise_epoch(
        &self,
        object_id: &str,
        node: Uuid,
        at_least: u64,
    ) -> Result<Option<u64>, RegistryError> {
        let at_least = i64::try_from(at_least).unwrap_or(i64::MAX);
        let Some((holder, held)) = self.lease_row(object_id).await? else {
            return Ok(None);
        };
        if holder != node {
            return Ok(None);
        }
        let epoch = if held < at_least {
            // Conditional on still holding it at the epoch just read, so
            // a takeover in between is never overwritten.
            let raised = Self::applied(
                self.session
                    .execute_unpaged(
                        &self.statements.raise_lease,
                        (at_least, object_id, node, at_least),
                    )
                    .await?,
            )?;
            if !raised {
                return Ok(self
                    .lease_row(object_id)
                    .await?
                    .filter(|(h, _)| *h == node)
                    .map(|(_, e)| e.max(1) as u64));
            }
            at_least
        } else {
            held
        };
        // The identity remembers the raise, so its next life lands above it.
        let rows = self
            .session
            .execute_unpaged(&self.statements.instance_id, (object_id,))
            .await?
            .into_rows_result()
            .map_err(rows_error)?;
        if let Some((scope, class, name)) = rows
            .maybe_first_row::<(Uuid, String, String)>()
            .map_err(rows_error)?
        {
            let last = self
                .instance_row(scope, &class, &name)
                .await?
                .and_then(|row| row.7)
                .unwrap_or(0);
            if last < epoch {
                self.session
                    .execute_unpaged(
                        &self.statements.instance_epoch,
                        (epoch, scope, &class, &name),
                    )
                    .await?;
            }
        }
        Ok(Some(epoch.max(1) as u64))
    }

    async fn deregister(&self, node: Uuid) -> Result<(), RegistryError> {
        self.depart(node, true).await
    }

    async fn release(&self, object_id: &str, node: Uuid) -> Result<(), RegistryError> {
        let released = Self::applied(
            self.session
                .execute_unpaged(&self.statements.delete_lease_if, (object_id, node))
                .await?,
        )?;
        if released {
            self.session
                .execute_unpaged(&self.statements.delete_lease_by_node, (node, object_id))
                .await?;
        }
        Ok(())
    }

    async fn set_alarm(
        &self,
        object_id: &str,
        own_key: &str,
        due_ms: i64,
    ) -> Result<(), RegistryError> {
        // One alarm per object: the previous bucket row goes first.
        self.drop_alarm(object_id).await?;
        self.session
            .execute_unpaged(
                &self.statements.alarm_insert,
                (&self.region, bucket(due_ms), due_ms, object_id, own_key),
            )
            .await?;
        self.session
            .execute_unpaged(
                &self.statements.alarm_by_object_insert,
                (object_id, due_ms, own_key),
            )
            .await?;
        self.session
            .execute_unpaged(
                &self.statements.alarm_bucket_insert,
                (&self.region, bucket(due_ms)),
            )
            .await?;
        Ok(())
    }

    async fn clear_alarm(&self, object_id: &str) -> Result<(), RegistryError> {
        self.drop_alarm(object_id).await
    }

    async fn due_alarms(&self, now_ms: i64, limit: usize) -> Result<Vec<AlarmRow>, RegistryError> {
        let mut due = Vec::new();
        for due_bucket in self.buckets(&self.statements.alarm_buckets).await? {
            if due_bucket > bucket(now_ms) || due.len() >= limit {
                break;
            }
            let rows = self
                .session
                .execute_unpaged(
                    &self.statements.alarms_due,
                    (&self.region, due_bucket, now_ms),
                )
                .await?
                .into_rows_result()
                .map_err(rows_error)?;
            let mut any = false;
            for row in rows.rows::<(i64, String, String)>().map_err(rows_error)? {
                let (due_ms, object_id, own_key) = row.map_err(rows_error)?;
                any = true;
                if due.len() < limit {
                    due.push(AlarmRow {
                        object_id,
                        own_key,
                        due_ms,
                    });
                }
            }
            // A bucket wholly in the past with nothing due is empty:
            // forget it so the next sweep skips it.
            if !any && due_bucket < bucket(now_ms) {
                self.session
                    .execute_unpaged(
                        &self.statements.alarm_bucket_delete,
                        (&self.region, due_bucket),
                    )
                    .await?;
            }
        }
        Ok(due)
    }

    async fn list_instances(
        &self,
        scopes: &[Uuid],
        class: &str,
        prefix: &str,
        limit: usize,
        offset: usize,
    ) -> Result<(u64, Vec<ObjectInstance>), RegistryError> {
        // One partition per scope, names in clustering order; the pages
        // are cut here, since the wire asks by offset.
        let mut all: Vec<(Uuid, InstanceTuple)> = Vec::new();
        for scope in scopes {
            let rows = if prefix.is_empty() {
                self.session
                    .execute_unpaged(&self.statements.instances_all, (scope, class))
                    .await?
            } else {
                self.session
                    .execute_unpaged(
                        &self.statements.instances_range,
                        (scope, class, prefix, prefix_end(prefix)),
                    )
                    .await?
            }
            .into_rows_result()
            .map_err(rows_error)?;
            for row in rows.rows::<InstanceTuple>().map_err(rows_error)? {
                all.push((*scope, row.map_err(rows_error)?));
            }
        }
        all.sort_by(|a, b| a.1.0.cmp(&b.1.0));
        let total = all.len() as u64;
        let mut page = Vec::new();
        for (scope, row) in all.into_iter().skip(offset).take(limit) {
            page.push(self.describe(scope, class, row).await?);
        }
        Ok((total, page))
    }

    async fn instance(
        &self,
        scope: Uuid,
        class: &str,
        name: &str,
    ) -> Result<Option<ObjectInstance>, RegistryError> {
        match self.instance_row(scope, class, name).await? {
            Some(row) => Ok(Some(self.describe(scope, class, row).await?)),
            None => Ok(None),
        }
    }

    async fn count_instances(&self, scopes: &[Uuid]) -> Result<Vec<ClassCount>, RegistryError> {
        // The fold rides the same partition read the count pays for.
        let mut counts: std::collections::BTreeMap<String, (u64, i64)> = Default::default();
        for scope in scopes {
            let classes = self
                .session
                .execute_unpaged(&self.statements.classes, (scope,))
                .await?
                .into_rows_result()
                .map_err(rows_error)?;
            let classes: Vec<String> = classes
                .rows::<(String,)>()
                .map_err(rows_error)?
                .map(|row| row.map(|(class,)| class))
                .collect::<Result<Vec<_>, _>>()
                .map_err(rows_error)?;
            for class in classes {
                let rows = self
                    .session
                    .execute_unpaged(&self.statements.instances_all, (scope, &class))
                    .await?
                    .into_rows_result()
                    .map_err(rows_error)?;
                let mut count = 0u64;
                let mut live: Vec<String> = Vec::new();
                for row in rows.rows::<InstanceTuple>().map_err(rows_error)? {
                    let row = row.map_err(rows_error)?;
                    count += 1;
                    if row.6.is_none()
                        && let Some(id) = row.1
                    {
                        live.push(id);
                    }
                }
                if count == 0 {
                    continue;
                }
                let fold =
                    actias_common::directory_identity::checksum(live.iter().map(String::as_str));
                let entry = counts.entry(class).or_insert((0, 0));
                entry.0 += count;
                entry.1 ^= fold;
            }
        }
        Ok(counts
            .into_iter()
            .map(|(class, (count, identities))| ClassCount {
                class,
                count,
                identities,
            })
            .collect())
    }

    async fn tombstone(
        &self,
        scope: Uuid,
        class: &str,
        name: &str,
        object_id: &str,
        only_if_expired: bool,
        now_ms: i64,
    ) -> Result<Option<u64>, RegistryError> {
        let Some(row) = self.instance_row(scope, class, name).await? else {
            return Ok(None);
        };
        if row.6.is_some() {
            return Ok(None);
        }
        let hash = if object_id.is_empty() {
            row.1.clone().unwrap_or_default()
        } else {
            object_id.to_owned()
        };
        if only_if_expired {
            let expired = row.5.is_some_and(|at| at <= now_ms);
            if !expired || self.alarm_of(&hash).await?.is_some() {
                return Ok(None);
            }
        }
        // The commit point: the tombstone and the epoch it happens
        // under, conditional on nobody else having tombstoned first.
        let epoch = next_epoch(row.7.unwrap_or(0));
        let applied = Self::applied(
            self.session
                .execute_unpaged(
                    &self.statements.instance_tombstone,
                    (now_ms, epoch, scope, class, name),
                )
                .await?,
        )?;
        if !applied {
            return Ok(None);
        }
        if let Some(old) = row.5 {
            self.drop_expiry(old, scope, class, name).await?;
        }
        self.session
            .execute_unpaged(
                &self.statements.tombstone_insert,
                (&self.region, now_ms, scope, class, name),
            )
            .await?;
        Ok(Some(epoch.max(1) as u64))
    }

    async fn purge(
        &self,
        scope: Uuid,
        class: &str,
        name: &str,
        object_id: &str,
    ) -> Result<(), RegistryError> {
        self.free_lease(object_id).await?;
        self.drop_alarm(object_id).await?;
        if let Some(row) = self.instance_row(scope, class, name).await?
            && let Some(deleted_at) = row.6
        {
            self.session
                .execute_unpaged(
                    &self.statements.tombstone_delete,
                    (&self.region, deleted_at, scope, class, name),
                )
                .await?;
            self.session
                .execute_unpaged(&self.statements.instance_delete, (scope, class, name))
                .await?;
            if let Some(id) = row.1 {
                self.session
                    .execute_unpaged(&self.statements.instance_id_delete, (id,))
                    .await?;
            }
        }
        Ok(())
    }

    async fn rollback_admission(
        &self,
        scope: Uuid,
        class: &str,
        name: &str,
        object_id: &str,
    ) -> Result<(), RegistryError> {
        self.free_lease(object_id).await?;
        if let Some(row) = self.instance_row(scope, class, name).await? {
            if let Some(old) = row.5 {
                self.drop_expiry(old, scope, class, name).await?;
            }
            if let Some(deleted_at) = row.6 {
                self.session
                    .execute_unpaged(
                        &self.statements.tombstone_delete,
                        (&self.region, deleted_at, scope, class, name),
                    )
                    .await?;
            }
            self.session
                .execute_unpaged(&self.statements.instance_delete, (scope, class, name))
                .await?;
            if let Some(id) = row.1 {
                self.session
                    .execute_unpaged(&self.statements.instance_id_delete, (id,))
                    .await?;
            }
        }
        Ok(())
    }

    async fn take_departure(&self) -> Result<Option<Departed>, RegistryError> {
        let rows = self
            .session
            .execute_unpaged(&self.statements.departures, (&self.region,))
            .await?
            .into_rows_result()
            .map_err(rows_error)?;
        let departures: Vec<(Uuid, bool, Vec<String>)> = rows
            .rows::<(Uuid, bool, Option<Vec<String>>)>()
            .map_err(rows_error)?
            .map(|row| row.map(|(node, drained, ids)| (node, drained, ids.unwrap_or_default())))
            .collect::<Result<Vec<_>, _>>()
            .map_err(rows_error)?;
        for (node, drained, object_ids) in departures {
            // A drained departure carries no repair obligation; it is
            // taken so the partition does not grow, and not handed out.
            let taken = Self::applied(
                self.session
                    .execute_unpaged(&self.statements.departure_take, (&self.region, node))
                    .await?,
            )?;
            if !taken || drained {
                continue;
            }
            let mut instances = Vec::new();
            for object_id in object_ids {
                let rows = self
                    .session
                    .execute_unpaged(&self.statements.instance_id, (&object_id,))
                    .await?
                    .into_rows_result()
                    .map_err(rows_error)?;
                if let Some((scope_id, class, name)) = rows
                    .maybe_first_row::<(Uuid, String, String)>()
                    .map_err(rows_error)?
                {
                    instances.push(HeldIdentity {
                        scope_id,
                        class,
                        name,
                    });
                }
            }
            return Ok(Some(Departed {
                node_id: node,
                instances,
            }));
        }
        Ok(None)
    }

    async fn unfinished_deletions(
        &self,
        now_ms: i64,
        limit: usize,
    ) -> Result<Vec<DeletionRow>, RegistryError> {
        let rows = self
            .session
            .execute_unpaged(&self.statements.tombstones, (&self.region, now_ms))
            .await?
            .into_rows_result()
            .map_err(rows_error)?;
        let mut out = Vec::new();
        for row in rows
            .rows::<(i64, Uuid, String, String)>()
            .map_err(rows_error)?
        {
            if out.len() >= limit {
                break;
            }
            let (_, scope, class, name) = row.map_err(rows_error)?;
            // The marker's epoch is minted above everything the identity
            // shipped and remembered, so asking twice climbs.
            let last = self
                .instance_row(scope, &class, &name)
                .await?
                .and_then(|row| row.7)
                .unwrap_or(0);
            let epoch = next_epoch(last);
            self.session
                .execute_unpaged(
                    &self.statements.instance_epoch,
                    (epoch, scope, &class, &name),
                )
                .await?;
            out.push(DeletionRow {
                scope_id: scope.to_string(),
                class,
                name,
                epoch: epoch.max(1) as u64,
            });
        }
        Ok(out)
    }

    async fn due_expiries(
        &self,
        now_ms: i64,
        limit: usize,
    ) -> Result<Vec<ExpiryRow>, RegistryError> {
        let mut due = Vec::new();
        for due_bucket in self.buckets(&self.statements.expiry_buckets).await? {
            if due_bucket > bucket(now_ms) || due.len() >= limit {
                break;
            }
            let rows = self
                .session
                .execute_unpaged(
                    &self.statements.expiries_due,
                    (&self.region, due_bucket, now_ms),
                )
                .await?
                .into_rows_result()
                .map_err(rows_error)?;
            let mut any = false;
            for row in rows
                .rows::<(i64, Uuid, String, String)>()
                .map_err(rows_error)?
            {
                let (_, scope, class, name) = row.map_err(rows_error)?;
                any = true;
                if due.len() >= limit {
                    continue;
                }
                // Not tombstoned, still due, and not waiting on an alarm:
                // futures block expiry.
                let Some(row) = self.instance_row(scope, &class, &name).await? else {
                    continue;
                };
                if row.6.is_some() || !row.5.is_some_and(|at| at <= now_ms) {
                    continue;
                }
                if let Some(id) = &row.1
                    && self.alarm_of(id).await?.is_some()
                {
                    continue;
                }
                due.push(ExpiryRow {
                    scope_id: scope.to_string(),
                    class,
                    name,
                });
            }
            if !any && due_bucket < bucket(now_ms) {
                self.session
                    .execute_unpaged(
                        &self.statements.expiry_bucket_delete,
                        (&self.region, due_bucket),
                    )
                    .await?;
            }
        }
        Ok(due)
    }

    async fn reap_expired(&self, cutoff_ms: i64) -> Result<(), RegistryError> {
        let dead: BTreeSet<Uuid> = self
            .live_nodes(i64::MIN)
            .await?
            .into_iter()
            .filter(|node| node.last_heartbeat_ms <= cutoff_ms)
            .map(|node| node.id)
            .collect();
        for node in dead {
            self.depart(node, false).await?;
        }
        Ok(())
    }
}
