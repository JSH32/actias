//! The storage contract: what any placement backend must answer, spoken
//! in the service's own proto types where one fits and in plain rows
//! where none does. The rpc layer holds a `dyn PlacementStore` and never
//! learns which backend is behind it; the conformance suite in
//! `registry.rs` is the contract's test form, and every backend runs all
//! of it.
//!
//! Time crosses the contract as unix milliseconds: the service computes
//! the liveness cutoff and the "now" of a sweep once, and a backend
//! compares against them in whatever type it stores.

use actias_common::thiserror;
use tonic::Status;
use uuid::Uuid;

use crate::proto_node_registry::{
    AcquireLeaseRequest, AlarmRow, ClassCount, DeletionRow, ExpiryRow, Lease, ObjectInstance,
};

/// What can fail inside the store. The [`From`] impl below is the one
/// place deciding what the wire sees; raw store detail stops at tracing.
#[derive(thiserror::Error, Debug)]
pub enum RegistryError {
    #[error("placement store query failed: {0}")]
    Store(String),
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

/// One membership row.
#[derive(Clone, Debug)]
pub struct NodeRow {
    pub id: Uuid,
    pub address: String,
    pub capabilities: Vec<String>,
    pub load: i32,
    pub registered_ms: i64,
    pub last_heartbeat_ms: i64,
}

/// The identity preimage a claim carries: (scope, script).
pub type Identity = (Uuid, Uuid);

/// One identity the dead node held, as the directory sweep needs it.
#[derive(Clone, Debug)]
pub struct HeldIdentity {
    pub scope_id: Uuid,
    pub class: String,
    pub name: String,
}

/// A departure handed out by [`PlacementStore::take_departure`].
#[derive(Clone, Debug)]
pub struct Departed {
    pub node_id: Uuid,
    pub instances: Vec<HeldIdentity>,
}

/// One placement backend. Semantics every implementation must keep:
///
/// - A lease is held by exactly one node, and only a claim against a
///   holder past the cutoff frees it; a re-claim by the holder is a
///   success that keeps the epoch. A claim that frees a dead holder
///   records that holder's departure first.
/// - Epochs move only forward for an identity: a takeover, a tombstone
///   and a rebirth each land above everything the identity held.
/// - The instance directory outlives leases; a tombstoned identity
///   refuses claims until purged.
/// - Alarms outlive their holders; a set replaces.
/// - A departure is handed out at most once.
/// - A forwarding row wins over a claim: while one stands, no lease is
///   taken here and the claim answers the region.
#[async_trait::async_trait]
pub trait PlacementStore: Send + Sync {
    async fn register(&self, address: &str, capabilities: &[String])
    -> Result<Uuid, RegistryError>;

    /// Records a beat and the load; `false` when the node is past the
    /// cutoff or unknown, which must not resurrect it.
    async fn heartbeat(&self, node: Uuid, load: i32, cutoff_ms: i64)
    -> Result<bool, RegistryError>;

    async fn live_nodes(&self, cutoff_ms: i64) -> Result<Vec<NodeRow>, RegistryError>;

    async fn node(&self, id: Uuid, cutoff_ms: i64) -> Result<Option<NodeRow>, RegistryError>;

    /// The conditional claim and everything it settles: the directory
    /// record when `identity` is given, the holder, the epoch.
    async fn claim(
        &self,
        request: &AcquireLeaseRequest,
        node: Uuid,
        identity: Option<Identity>,
        cutoff_ms: i64,
    ) -> Result<Lease, RegistryError>;

    /// Who holds `object_id` and under which epoch, dead holders reading
    /// as nobody.
    async fn holder(
        &self,
        object_id: &str,
        cutoff_ms: i64,
    ) -> Result<Option<(Uuid, u64)>, RegistryError>;

    /// Raises `node`'s epoch on `object_id` to at least `at_least`,
    /// remembering it on the identity; the epoch as it stands after, or
    /// [`None`] when `node` does not hold the object.
    async fn raise_epoch(
        &self,
        object_id: &str,
        node: Uuid,
        at_least: u64,
    ) -> Result<Option<u64>, RegistryError>;

    /// The graceful goodbye: a drained departure, then the node and its
    /// leases go.
    async fn deregister(&self, node: Uuid) -> Result<(), RegistryError>;

    /// Frees the lease when `node` holds it; anyone else's release is a
    /// no-op.
    async fn release(&self, object_id: &str, node: Uuid) -> Result<(), RegistryError>;

    /// Records that `object_id`, born here, lives in `region` now; a
    /// claim here answers the region instead of a lease from then on.
    /// Replaces an earlier row.
    async fn set_move(&self, object_id: &str, region: &str) -> Result<(), RegistryError>;

    /// The region `object_id` was moved to, or [`None`] when it is at
    /// its birth region.
    async fn get_move(&self, object_id: &str) -> Result<Option<String>, RegistryError>;

    /// The object is home again; a missing row is not an error.
    async fn clear_move(&self, object_id: &str) -> Result<(), RegistryError>;

    async fn set_alarm(
        &self,
        object_id: &str,
        own_key: &str,
        due_ms: i64,
    ) -> Result<(), RegistryError>;

    async fn clear_alarm(&self, object_id: &str) -> Result<(), RegistryError>;

    /// Alarms due at `now_ms`, oldest first, at most `limit`.
    async fn due_alarms(&self, now_ms: i64, limit: usize) -> Result<Vec<AlarmRow>, RegistryError>;

    /// One class of the given scopes, names starting with `prefix`,
    /// ordered by name: the total and one page.
    async fn list_instances(
        &self,
        scopes: &[Uuid],
        class: &str,
        prefix: &str,
        limit: usize,
        offset: usize,
    ) -> Result<(u64, Vec<ObjectInstance>), RegistryError>;

    async fn instance(
        &self,
        scope: Uuid,
        class: &str,
        name: &str,
    ) -> Result<Option<ObjectInstance>, RegistryError>;

    /// Per class, how many identities the scopes hold and the fold of
    /// the live ones' ids (`actias_common::directory_identity`).
    async fn count_instances(&self, scopes: &[Uuid]) -> Result<Vec<ClassCount>, RegistryError>;

    /// The deletion commit point: tombstones the identity and mints the
    /// epoch everything after runs under. [`None`] when nothing was
    /// tombstoned: already gone, or `only_if_expired` and not expired
    /// or holding an alarm.
    async fn tombstone(
        &self,
        scope: Uuid,
        class: &str,
        name: &str,
        object_id: &str,
        only_if_expired: bool,
        now_ms: i64,
    ) -> Result<Option<u64>, RegistryError>;

    /// The end of a deletion, idempotent: lease, alarm and the
    /// tombstoned row go.
    async fn purge(
        &self,
        scope: Uuid,
        class: &str,
        name: &str,
        object_id: &str,
    ) -> Result<(), RegistryError>;

    /// Unwinds a refused admission: lease and row go, whatever their state.
    async fn rollback_admission(
        &self,
        scope: Uuid,
        class: &str,
        name: &str,
        object_id: &str,
    ) -> Result<(), RegistryError>;

    /// One undrained departure, handed out once, its held identities
    /// resolved (a hash with no row simply does not appear).
    async fn take_departure(&self) -> Result<Option<Departed>, RegistryError>;

    /// Tombstoned rows older than `now_ms` whose purge never ran, each
    /// with a marker epoch above everything the identity shipped.
    async fn unfinished_deletions(
        &self,
        now_ms: i64,
        limit: usize,
    ) -> Result<Vec<DeletionRow>, RegistryError>;

    /// Identities past their expiry at `now_ms`, not tombstoned, holding
    /// no alarm.
    async fn due_expiries(
        &self,
        now_ms: i64,
        limit: usize,
    ) -> Result<Vec<ExpiryRow>, RegistryError>;

    /// Deletes every node past the cutoff, each leaving an undrained
    /// departure with what it held.
    async fn reap_expired(&self, cutoff_ms: i64) -> Result<(), RegistryError>;
}

/// The next epoch for an identity whose last is `last`: the clock, or
/// one past what it held, whichever is later. The clock gives
/// monotonicity across a fleet without coordination; the identity's own
/// memory closes the skew.
pub fn next_epoch(last: i64) -> i64 {
    now_ms().max(last + 1)
}

/// Unix milliseconds now.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
