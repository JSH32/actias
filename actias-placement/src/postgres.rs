//! The postgres backend, the small-stack default: one transaction where
//! two rows must move together, a sequence for epochs, the cascade from
//! nodes to leases doing age-out and lease expiry as one deletion.

use std::str::FromStr;

use sqlx::types::chrono::{DateTime, Utc};
use sqlx::{Pool, Postgres};
use uuid::Uuid;

use crate::proto_node_registry::{
    AcquireLeaseRequest, AlarmRow, ClassCount, DeletionRow, ExpiryRow, Lease, ObjectInstance,
};
use crate::store::{
    Departed, HeldIdentity, Identity, NodeRow, PlacementStore, RegistryError, next_epoch,
};

impl From<sqlx::Error> for RegistryError {
    fn from(error: sqlx::Error) -> Self {
        RegistryError::Store(error.to_string())
    }
}

/// One membership row as postgres hands it back.
#[derive(sqlx::FromRow)]
struct DbNode {
    id: Uuid,
    address: String,
    capabilities: Vec<String>,
    load: i32,
    registered: DateTime<Utc>,
    last_heartbeat: DateTime<Utc>,
}

impl From<DbNode> for NodeRow {
    fn from(node: DbNode) -> Self {
        NodeRow {
            id: node.id,
            address: node.address,
            capabilities: node.capabilities,
            load: node.load,
            registered_ms: node.registered.timestamp_millis(),
            last_heartbeat_ms: node.last_heartbeat.timestamp_millis(),
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

/// A cutoff or a now, as the timestamp postgres compares against.
fn at(ms: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(ms).unwrap_or_else(Utc::now)
}

/// The column tuple every instance query selects, in select-list order.
type InstanceTuple = (
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
    String,
);

fn instance(row: InstanceTuple) -> ObjectInstance {
    let (
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
        object_id,
    ) = row;
    ObjectInstance {
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
        object_id,
    }
}

/// The select list behind [`instance`]: the lifetime join rides the
/// hash column, so rows without one read as cold and alarmless.
const INSTANCE_SELECT: &str = "SELECT i.scope_id, i.class, i.name, i.script_id,
        (EXTRACT(EPOCH FROM i.created) * 1000)::BIGINT,
        COALESCE((EXTRACT(EPOCH FROM i.expire_at) * 1000)::BIGINT, 0),
        COALESCE((EXTRACT(EPOCH FROM i.deleted_at) * 1000)::BIGINT, 0),
        COALESCE(a.due_ms, 0),
        COALESCE(l.node_id::text, ''),
        COALESCE(i.created_by, ''),
        COALESCE(i.object_id, '')
 FROM object_instances i
 LEFT JOIN object_alarms a ON a.object_id = i.object_id
 LEFT JOIN leases l ON l.object_id = i.object_id";

pub struct PostgresStore {
    database: Pool<Postgres>,
}

impl PostgresStore {
    pub fn new(database: Pool<Postgres>) -> Self {
        Self { database }
    }

    /// Writes one node's departure record inside the caller's
    /// transaction, before the node row's deletion cascades its leases
    /// away. Idempotent: a raced double-exit keeps the first record.
    async fn record_departure(
        tx: &mut sqlx::Transaction<'_, Postgres>,
        node_id: Uuid,
        drained: bool,
    ) -> Result<(), RegistryError> {
        sqlx::query(
            "INSERT INTO node_departures (node_id, drained, object_ids)
             SELECT $1, $2,
                    COALESCE((SELECT array_agg(object_id) FROM leases
                              WHERE node_id = $1), '{}')
             ON CONFLICT (node_id) DO NOTHING",
        )
        .bind(node_id)
        .bind(drained)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl PlacementStore for PostgresStore {
    async fn register(
        &self,
        address: &str,
        capabilities: &[String],
    ) -> Result<Uuid, RegistryError> {
        Ok(sqlx::query_scalar(
            "INSERT INTO nodes (address, capabilities) VALUES ($1, $2) RETURNING id",
        )
        .bind(address)
        .bind(capabilities)
        .fetch_one(&self.database)
        .await?)
    }

    async fn heartbeat(
        &self,
        node: Uuid,
        load: i32,
        cutoff_ms: i64,
    ) -> Result<bool, RegistryError> {
        // An aged-out node must not resurrect by beating: only a row still
        // inside the ttl accepts the update.
        let updated = sqlx::query(
            "UPDATE nodes SET last_heartbeat = now(), load = $2
             WHERE id = $1 AND last_heartbeat > $3",
        )
        .bind(node)
        .bind(load)
        .bind(at(cutoff_ms))
        .execute(&self.database)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    async fn live_nodes(&self, cutoff_ms: i64) -> Result<Vec<NodeRow>, RegistryError> {
        let nodes = sqlx::query_as::<_, DbNode>(
            "SELECT * FROM nodes WHERE last_heartbeat > $1 ORDER BY registered",
        )
        .bind(at(cutoff_ms))
        .fetch_all(&self.database)
        .await?;
        Ok(nodes.into_iter().map(NodeRow::from).collect())
    }

    async fn node(&self, id: Uuid, cutoff_ms: i64) -> Result<Option<NodeRow>, RegistryError> {
        Ok(
            sqlx::query_as::<_, DbNode>(
                "SELECT * FROM nodes WHERE id = $1 AND last_heartbeat > $2",
            )
            .bind(id)
            .bind(at(cutoff_ms))
            .fetch_optional(&self.database)
            .await?
            .map(NodeRow::from),
        )
    }

    async fn claim(
        &self,
        request: &AcquireLeaseRequest,
        node_id: Uuid,
        identity: Option<Identity>,
        cutoff_ms: i64,
    ) -> Result<Lease, RegistryError> {
        // A tombstoned identity refuses claims until the janitor finishes:
        // deletion in progress is not a home. Recreation becomes legal the
        // moment the directory row is purged.
        let mut last_epoch = 0i64;
        if let Some((scope_id, _)) = identity {
            let row: Option<(bool, i64)> = sqlx::query_as(
                "SELECT deleted_at IS NOT NULL, last_epoch FROM object_instances
                 WHERE scope_id = $1 AND class = $2 AND name = $3",
            )
            .bind(scope_id)
            .bind(&request.class)
            .bind(&request.name)
            .fetch_optional(&self.database)
            .await?;
            if let Some((deleting, last)) = row {
                if deleting {
                    return Err(RegistryError::Deleting);
                }
                last_epoch = last;
            }
        }

        // A forwarding row wins over any claim: the object was born here
        // and lives elsewhere now; the caller learns where and forwards.
        if let Some(region) = self.get_move(&request.object_id).await? {
            return Ok(Lease {
                object_id: request.object_id.clone(),
                node_id: String::new(),
                acquired: false,
                epoch: 0,
                fresh: false,
                moved_to: region,
            });
        }

        // The conditional claim: exactly one row per object, first insert
        // wins, a re-claim by the current holder is a no-op success. A
        // refused claim checks the incumbent's own pulse instead of
        // sweeping the whole table: a dead incumbent is evicted (the same
        // cascade age-out uses) and the claim retried once, so failover
        // stays instant without a DELETE on every claim. The full sweep
        // runs on its own timer.
        let mut claimed = sqlx::query(
            "INSERT INTO leases (object_id, node_id, epoch)
             VALUES ($1, $2, $3)
             ON CONFLICT (object_id) DO NOTHING",
        )
        .bind(&request.object_id)
        .bind(node_id)
        .bind(next_epoch(last_epoch))
        .execute(&self.database)
        .await?;

        if claimed.rows_affected() == 0 {
            let stale: Option<(Uuid, i64)> = sqlx::query_as(
                "SELECT l.node_id, l.epoch FROM leases l
                 LEFT JOIN nodes n ON n.id = l.node_id
                 WHERE l.object_id = $1
                   AND (n.id IS NULL OR n.last_heartbeat <= $2)",
            )
            .bind(&request.object_id)
            .bind(at(cutoff_ms))
            .fetch_optional(&self.database)
            .await?;
            if let Some((dead, held)) = stale {
                // An eviction is an unclean exit observed early: the
                // departure record must capture the dead node's leases
                // before the cascade frees them, same as the reaper.
                let mut tx = self.database.begin().await?;
                Self::record_departure(&mut tx, dead, false).await?;
                sqlx::query("DELETE FROM nodes WHERE id = $1")
                    .bind(dead)
                    .execute(&mut *tx)
                    .await?;
                // The cascade freed the lease unless the row was already
                // orphaned; clear it either way before retrying.
                sqlx::query("DELETE FROM leases WHERE object_id = $1 AND node_id = $2")
                    .bind(&request.object_id)
                    .bind(dead)
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await?;
                // A takeover lands above what the dead holder held.
                claimed = sqlx::query(
                    "INSERT INTO leases (object_id, node_id, epoch)
                     VALUES ($1, $2, $3)
                     ON CONFLICT (object_id) DO NOTHING",
                )
                .bind(&request.object_id)
                .bind(node_id)
                .bind(next_epoch(last_epoch.max(held)))
                .execute(&self.database)
                .await?;
            }
        }

        // The claim carries its preimage; the directory keeps it so the
        // data stays enumerable after the declaring revision is gone.
        // Every claim restates the lifetime (touch refreshes, policy
        // changes apply on next touch, 0 clears); the creator is kept
        // from the first claim only.
        let mut fresh = false;
        if let Some((scope_id, script_id)) = identity {
            // xmax = 0 marks a row this statement inserted rather than
            // updated: whether the identity is fresh, which is the only
            // kind an admission gate examines.
            fresh = sqlx::query_scalar(
                "INSERT INTO object_instances
                     (scope_id, class, name, script_id, object_id,
                      created_by, expire_at, last_epoch)
                 VALUES ($1, $2, $3, $4, $5, NULLIF($6, ''),
                         CASE WHEN $7 > 0
                              THEN now() + make_interval(secs => $7)
                              ELSE NULL END,
                         COALESCE((SELECT epoch FROM leases WHERE object_id = $5), 0))
                 ON CONFLICT (scope_id, class, name) DO UPDATE
                 SET object_id = COALESCE(object_instances.object_id,
                                          EXCLUDED.object_id),
                     expire_at = EXCLUDED.expire_at,
                     last_epoch = GREATEST(object_instances.last_epoch,
                                           EXCLUDED.last_epoch)
                 RETURNING (xmax = 0)",
            )
            .bind(scope_id)
            .bind(&request.class)
            .bind(&request.name)
            .bind(script_id)
            .bind(&request.object_id)
            .bind(&request.created_by)
            .bind(request.expire_secs.min(i64::MAX as u64) as f64)
            .fetch_one(&self.database)
            .await?;
        }

        // The lease row answers both questions: who holds it, and under
        // which epoch. A claim that won minted its own with the insert;
        // a claim that lost reads the incumbent's, which is what a
        // re-claim by the holder must get back.
        let held: Option<(Uuid, i64)> =
            sqlx::query_as("SELECT node_id, epoch FROM leases WHERE object_id = $1")
                .bind(&request.object_id)
                .fetch_optional(&self.database)
                .await?;
        let (holder, epoch) = held.ok_or(RegistryError::ClaimRaced)?;
        let acquired = claimed.rows_affected() == 1 || holder == node_id;

        Ok(Lease {
            object_id: request.object_id.clone(),
            node_id: holder.to_string(),
            acquired,
            epoch: epoch.max(1) as u64,
            fresh,
            moved_to: String::new(),
        })
    }

    async fn set_move(&self, object_id: &str, region: &str) -> Result<(), RegistryError> {
        sqlx::query(
            "INSERT INTO moves (object_id, region) VALUES ($1, $2)
             ON CONFLICT (object_id) DO UPDATE SET region = EXCLUDED.region, moved_at = now()",
        )
        .bind(object_id)
        .bind(region)
        .execute(&self.database)
        .await?;
        Ok(())
    }

    async fn get_move(&self, object_id: &str) -> Result<Option<String>, RegistryError> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT region FROM moves WHERE object_id = $1")
                .bind(object_id)
                .fetch_optional(&self.database)
                .await?;
        Ok(row.map(|(region,)| region))
    }

    async fn clear_move(&self, object_id: &str) -> Result<(), RegistryError> {
        sqlx::query("DELETE FROM moves WHERE object_id = $1")
            .bind(object_id)
            .execute(&self.database)
            .await?;
        Ok(())
    }

    async fn holder(
        &self,
        object_id: &str,
        cutoff_ms: i64,
    ) -> Result<Option<(Uuid, u64)>, RegistryError> {
        // A dead holder must read as unheld, exactly as a claim would
        // treat it; the liveness filter does it without a delete, and
        // the sweep timer does the physical ageing.
        let held: Option<(Uuid, i64)> = sqlx::query_as(
            "SELECT l.node_id, l.epoch FROM leases l
             JOIN nodes n ON n.id = l.node_id AND n.last_heartbeat > $2
             WHERE l.object_id = $1",
        )
        .bind(object_id)
        .bind(at(cutoff_ms))
        .fetch_optional(&self.database)
        .await?;
        Ok(held.map(|(node, epoch)| (node, epoch.max(1) as u64)))
    }

    async fn raise_epoch(
        &self,
        object_id: &str,
        node: Uuid,
        at_least: u64,
    ) -> Result<Option<u64>, RegistryError> {
        let at_least = i64::try_from(at_least).unwrap_or(i64::MAX);
        let mut tx = self.database.begin().await?;
        let epoch: Option<i64> = sqlx::query_scalar(
            "UPDATE leases SET epoch = GREATEST(epoch, $3)
             WHERE object_id = $1 AND node_id = $2
             RETURNING epoch",
        )
        .bind(object_id)
        .bind(node)
        .bind(at_least)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(epoch) = epoch else {
            tx.rollback().await?;
            return Ok(None);
        };
        sqlx::query(
            "UPDATE object_instances SET last_epoch = GREATEST(last_epoch, $2)
             WHERE object_id = $1",
        )
        .bind(object_id)
        .bind(epoch)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Some(epoch.max(1) as u64))
    }

    async fn deregister(&self, node: Uuid) -> Result<(), RegistryError> {
        // A goodbye is age-out brought forward: the same deletion, the
        // same lease-freeing cascade, none of the waiting. The departure
        // records drained = true, because deregistration only happens at
        // the end of a graceful stop, after the shippers and the
        // directory syncer flushed to zero.
        let mut tx = self.database.begin().await?;
        Self::record_departure(&mut tx, node, true).await?;
        sqlx::query("DELETE FROM nodes WHERE id = $1")
            .bind(node)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn release(&self, object_id: &str, node: Uuid) -> Result<(), RegistryError> {
        sqlx::query("DELETE FROM leases WHERE object_id = $1 AND node_id = $2")
            .bind(object_id)
            .bind(node)
            .execute(&self.database)
            .await?;
        Ok(())
    }

    async fn set_alarm(
        &self,
        object_id: &str,
        own_key: &str,
        due_ms: i64,
    ) -> Result<(), RegistryError> {
        sqlx::query(
            "INSERT INTO object_alarms (object_id, own_key, due_ms) VALUES ($1, $2, $3)
             ON CONFLICT (object_id) DO UPDATE SET own_key = $2, due_ms = $3",
        )
        .bind(object_id)
        .bind(own_key)
        .bind(due_ms)
        .execute(&self.database)
        .await?;
        Ok(())
    }

    async fn clear_alarm(&self, object_id: &str) -> Result<(), RegistryError> {
        sqlx::query("DELETE FROM object_alarms WHERE object_id = $1")
            .bind(object_id)
            .execute(&self.database)
            .await?;
        Ok(())
    }

    async fn due_alarms(&self, now_ms: i64, limit: usize) -> Result<Vec<AlarmRow>, RegistryError> {
        // Deliberately not filtered by holder liveness: a due alarm on a
        // dead node's object is exactly the row this query exists for.
        let rows: Vec<(String, String, i64)> = sqlx::query_as(
            "SELECT object_id, own_key, due_ms FROM object_alarms
             WHERE due_ms <= $1 ORDER BY due_ms LIMIT $2",
        )
        .bind(now_ms)
        .bind(limit as i64)
        .fetch_all(&self.database)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(object_id, own_key, due_ms)| AlarmRow {
                object_id,
                own_key,
                due_ms,
            })
            .collect())
    }

    async fn list_instances(
        &self,
        scopes: &[Uuid],
        class: &str,
        prefix: &str,
        limit: usize,
        offset: usize,
    ) -> Result<(u64, Vec<ObjectInstance>), RegistryError> {
        // An empty scope list matches no rows: `scope_id = ANY('{}')` is
        // false for every row, the safe default for a multi-tenant
        // listing.
        let pattern = like_prefix(prefix);
        let total: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM object_instances
             WHERE scope_id = ANY($1) AND class = $2 AND name LIKE $3",
        )
        .bind(scopes)
        .bind(class)
        .bind(&pattern)
        .fetch_one(&self.database)
        .await?;
        let rows: Vec<InstanceTuple> = sqlx::query_as(&format!(
            "{INSTANCE_SELECT}
             WHERE i.scope_id = ANY($1) AND i.class = $2 AND i.name LIKE $3
             ORDER BY i.class, i.name
             LIMIT $4 OFFSET $5"
        ))
        .bind(scopes)
        .bind(class)
        .bind(&pattern)
        .bind(limit as i64)
        .bind(offset as i64)
        .fetch_all(&self.database)
        .await?;
        Ok((
            total.max(0) as u64,
            rows.into_iter().map(instance).collect(),
        ))
    }

    async fn instance(
        &self,
        scope: Uuid,
        class: &str,
        name: &str,
    ) -> Result<Option<ObjectInstance>, RegistryError> {
        let row: Option<InstanceTuple> = sqlx::query_as(&format!(
            "{INSTANCE_SELECT}
             WHERE i.scope_id = $1 AND i.class = $2 AND i.name = $3"
        ))
        .bind(scope)
        .bind(class)
        .bind(name)
        .fetch_optional(&self.database)
        .await?;
        Ok(row.map(instance))
    }

    async fn count_instances(&self, scopes: &[Uuid]) -> Result<Vec<ClassCount>, RegistryError> {
        // The identity fold rides the same scan the count already pays
        // for: object ids are blake3 hex, so a fixed prefix of one IS a
        // hash of the identity, and the directory takes the same prefix
        // on its own rows. Live rows only, because a tombstoned
        // identity's row is a tombstone in the index too. An id that is
        // not hex contributes nothing rather than failing the count,
        // which is what the rust fold does with it as well.
        let rows: Vec<(String, i64, Option<i64>)> = sqlx::query_as(
            "SELECT class, count(*),
                    bit_xor(CASE
                        WHEN deleted_at IS NULL AND object_id ~ '^[0-9a-f]{15}'
                        THEN ('x' || substr(object_id, 1, 15))::bit(60)::bigint
                        ELSE 0 END)
             FROM object_instances
             WHERE scope_id = ANY($1)
             GROUP BY class ORDER BY class",
        )
        .bind(scopes)
        .fetch_all(&self.database)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(class, count, identities)| ClassCount {
                class,
                count: count.max(0) as u64,
                identities: identities.unwrap_or(0),
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
        // The commit point, one transaction: tombstone plus epoch bump.
        // The sweep's guard re-checks its predicate here, so a claim
        // that refreshed the row between query and tombstone wins. An
        // empty hash reads from the row: external callers name the
        // identity, only workers hash.
        let mut tx = self.database.begin().await?;
        let row: Option<(Option<String>, i64)> = sqlx::query_as(
            "SELECT object_id, last_epoch FROM object_instances
             WHERE scope_id = $1 AND class = $2 AND name = $3",
        )
        .bind(scope)
        .bind(class)
        .bind(name)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((stored_id, last_epoch)) = row else {
            tx.rollback().await?;
            return Ok(None);
        };
        let object_id = if object_id.is_empty() {
            stored_id.unwrap_or_default()
        } else {
            object_id.to_owned()
        };
        // Everything after the tombstone happens under a newer epoch
        // than any pre-deletion holder ever held: the lease's epoch is
        // the freshest memory while a holder lives, the row's after.
        let held: Option<i64> = sqlx::query_scalar("SELECT epoch FROM leases WHERE object_id = $1")
            .bind(&object_id)
            .fetch_optional(&mut *tx)
            .await?;
        let epoch = next_epoch(last_epoch.max(held.unwrap_or(0)));
        let tombstoned = sqlx::query(
            "UPDATE object_instances SET deleted_at = now(), last_epoch = $7
             WHERE scope_id = $1 AND class = $2 AND name = $3
               AND deleted_at IS NULL
               AND ($4 = false
                    OR (expire_at IS NOT NULL AND expire_at <= $6
                        AND NOT EXISTS (SELECT 1 FROM object_alarms a
                                        WHERE a.object_id = $5)))",
        )
        .bind(scope)
        .bind(class)
        .bind(name)
        .bind(only_if_expired)
        .bind(&object_id)
        .bind(at(now_ms))
        .bind(epoch)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        if !tombstoned {
            tx.rollback().await?;
            return Ok(None);
        }
        tx.commit().await?;
        Ok(Some(epoch.max(1) as u64))
    }

    async fn purge(
        &self,
        scope: Uuid,
        class: &str,
        name: &str,
        object_id: &str,
    ) -> Result<(), RegistryError> {
        // The end of the sequence, idempotent so the janitor can retry
        // it: the lease goes, then the directory row leaves the listing.
        // Alarms go too; a deleted object has no future obligations.
        sqlx::query("DELETE FROM leases WHERE object_id = $1")
            .bind(object_id)
            .execute(&self.database)
            .await?;
        sqlx::query("DELETE FROM object_alarms WHERE object_id = $1")
            .bind(object_id)
            .execute(&self.database)
            .await?;
        sqlx::query(
            "DELETE FROM object_instances
             WHERE scope_id = $1 AND class = $2 AND name = $3
               AND deleted_at IS NOT NULL",
        )
        .bind(scope)
        .bind(class)
        .bind(name)
        .execute(&self.database)
        .await?;
        Ok(())
    }

    async fn rollback_admission(
        &self,
        scope: Uuid,
        class: &str,
        name: &str,
        object_id: &str,
    ) -> Result<(), RegistryError> {
        // One transaction, mirroring the claim it unwinds. The lease
        // carries the epoch, so dropping it drops the residency and its
        // fence together; the sequence has moved on either way.
        let mut tx = self.database.begin().await?;
        sqlx::query("DELETE FROM leases WHERE object_id = $1")
            .bind(object_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "DELETE FROM object_instances
             WHERE scope_id = $1 AND class = $2 AND name = $3",
        )
        .bind(scope)
        .bind(class)
        .bind(name)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn take_departure(&self) -> Result<Option<Departed>, RegistryError> {
        // Taken and deleted in one statement. SKIP LOCKED is what lets
        // every node run this loop without coordinating: two sweepers
        // racing take different departures rather than the same one.
        let taken: Option<(Uuid, Vec<String>)> = sqlx::query_as(
            // The locking clause follows LIMIT, which postgres
            // requires; putting it before is a syntax error.
            "DELETE FROM node_departures
             WHERE node_id = (
                 SELECT node_id FROM node_departures
                 WHERE NOT drained
                 ORDER BY departed_at
                 LIMIT 1
                 FOR UPDATE SKIP LOCKED
             )
             RETURNING node_id, object_ids",
        )
        .fetch_optional(&self.database)
        .await?;
        let Some((node_id, object_ids)) = taken else {
            return Ok(None);
        };
        // The lease knows only the hash, and a directory delta is
        // written under the class's prefix, so the identity has to come
        // back. A hash with no row is an object whose identity was
        // already purged: nothing to repair.
        let rows: Vec<(Uuid, String, String)> = sqlx::query_as(
            "SELECT scope_id, class, name FROM object_instances
             WHERE object_id = ANY($1)",
        )
        .bind(&object_ids)
        .fetch_all(&self.database)
        .await?;
        Ok(Some(Departed {
            node_id,
            instances: rows
                .into_iter()
                .map(|(scope_id, class, name)| HeldIdentity {
                    scope_id,
                    class,
                    name,
                })
                .collect(),
        }))
    }

    async fn unfinished_deletions(
        &self,
        now_ms: i64,
        limit: usize,
    ) -> Result<Vec<DeletionRow>, RegistryError> {
        // Rows the tombstone committed but the purge never removed. The
        // marker's epoch is minted above everything the identity
        // remembers and remembered in turn, so asking twice climbs.
        let rows: Vec<(Uuid, String, String, i64)> = sqlx::query_as(
            "UPDATE object_instances i SET last_epoch = GREATEST(
                 i.last_epoch + 1,
                 (EXTRACT(EPOCH FROM now()) * 1000)::BIGINT)
             FROM (SELECT scope_id, class, name FROM object_instances
                   WHERE deleted_at IS NOT NULL AND deleted_at <= $1
                   ORDER BY deleted_at
                   LIMIT $2) due
             WHERE i.scope_id = due.scope_id AND i.class = due.class AND i.name = due.name
             RETURNING i.scope_id, i.class, i.name, i.last_epoch",
        )
        .bind(at(now_ms))
        .bind(limit as i64)
        .fetch_all(&self.database)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(scope_id, class, name, epoch)| DeletionRow {
                scope_id: scope_id.to_string(),
                class,
                name,
                epoch: epoch.max(1) as u64,
            })
            .collect())
    }

    async fn due_expiries(
        &self,
        now_ms: i64,
        limit: usize,
    ) -> Result<Vec<ExpiryRow>, RegistryError> {
        // Past due, not tombstoned, and not waiting: an alarm means the
        // instance has a future, and futures block expiry.
        let rows: Vec<(Uuid, String, String)> = sqlx::query_as(
            "SELECT i.scope_id, i.class, i.name
             FROM object_instances i
             LEFT JOIN object_alarms a ON a.object_id = i.object_id
             WHERE i.expire_at IS NOT NULL
               AND i.expire_at <= $1
               AND i.deleted_at IS NULL
               AND i.object_id IS NOT NULL
               AND a.object_id IS NULL
             ORDER BY i.expire_at
             LIMIT $2",
        )
        .bind(at(now_ms))
        .bind(limit as i64)
        .fetch_all(&self.database)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(scope_id, class, name)| ExpiryRow {
                scope_id: scope_id.to_string(),
                class,
                name,
            })
            .collect())
    }

    async fn reap_expired(&self, cutoff_ms: i64) -> Result<(), RegistryError> {
        // Ageing out is physical, so the table never accumulates a
        // graveyard, and lease expiry is the same deletion through the
        // cascade. Each reaped node leaves an undrained departure first.
        let mut tx = self.database.begin().await?;
        sqlx::query(
            "INSERT INTO node_departures (node_id, drained, object_ids)
             SELECT n.id, false,
                    COALESCE((SELECT array_agg(l.object_id) FROM leases l
                              WHERE l.node_id = n.id), '{}')
             FROM nodes n WHERE n.last_heartbeat <= $1
             ON CONFLICT (node_id) DO NOTHING",
        )
        .bind(at(cutoff_ms))
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM nodes WHERE last_heartbeat <= $1")
            .bind(at(cutoff_ms))
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}

/// Connects a pool, retrying while the database comes up; the service
/// typically races its datastore out of a cold start.
///
/// # Panics
/// Panics when the database never accepts; this runs at startup, where
/// dying loudly is the right outcome.
pub async fn connect(url: &str) -> Pool<Postgres> {
    for _ in 0..60 {
        if let Ok(pool) = Pool::<Postgres>::connect(url).await
            && sqlx::query("SELECT 1").execute(&pool).await.is_ok()
        {
            return pool;
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
    panic!("postgres did not accept a connection from DATABASE_URL");
}

/// The parse every id field shares.
pub fn uuid(field: &'static str, value: &str) -> Result<Uuid, RegistryError> {
    Uuid::from_str(value).map_err(|_| RegistryError::InvalidId(field))
}
