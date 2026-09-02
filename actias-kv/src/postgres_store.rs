//! The postgres backend, the default: pairs in one btree keyed
//! (project_id, namespace, key), so point gets and key-ordered listing
//! ride the index. TTL is an `expires_at` column: reads filter expired
//! rows out and report the remaining life by subtraction, and
//! [`PostgresStore::sweep`] reclaims them in the background, which is
//! the one job scylla's cell TTL did on its own.

use std::io::{self, Write};

use base64::{engine::general_purpose, read, write};
use sqlx::{PgPool, Row, postgres::PgRow};
use uuid::Uuid;

use crate::proto_kv_service::{
    ListNamespacesResponse, ListPairsResponse, Namespace, Pair, PairRequest, ValueType,
};
use crate::store::{DatabaseError, KvStore};

impl From<sqlx::Error> for DatabaseError {
    fn from(error: sqlx::Error) -> Self {
        DatabaseError::Backend(error.to_string())
    }
}

pub struct PostgresStore {
    pool: PgPool,
}

/// Connects a pool, retrying while the database comes up; the service
/// typically races its datastore out of a cold start.
///
/// # Panics
/// Panics when the database never accepts; this runs at startup, where
/// dying loudly is the right outcome.
pub async fn connect(url: &str) -> PgPool {
    for _ in 0..60 {
        if let Ok(pool) = PgPool::connect(url).await
            && sqlx::query("SELECT 1").execute(&pool).await.is_ok()
        {
            return pool;
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
    panic!("postgres did not accept a connection from DATABASE_URL");
}

impl PostgresStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn project_uuid(project_id: &str) -> Result<Uuid, DatabaseError> {
        Uuid::parse_str(project_id).map_err(|e| DatabaseError::Invalid(e.to_string()))
    }

    fn row_into_pair(row: &PgRow) -> Result<Pair, DatabaseError> {
        let column = |e: sqlx::Error| DatabaseError::Rows(e.to_string());
        let value_type: ValueType = row
            .try_get::<String, _>("type")
            .map_err(column)?
            .try_into()
            .map_err(DatabaseError::Invalid)?;

        Ok(Pair {
            ttl: row.try_get("ttl").map_err(column)?,
            project_id: row
                .try_get::<Uuid, _>("project_id")
                .map_err(column)?
                .to_string(),
            namespace: row.try_get("namespace").map_err(column)?,
            key: row.try_get("key").map_err(column)?,
            value: row.try_get("value").map_err(column)?,
            r#type: value_type.into(),
        })
    }

    /// Writes also register implicitly; both write paths funnel here so
    /// the registry insert exists exactly once.
    async fn register_namespace(
        &self,
        project_id: Uuid,
        namespace: &str,
    ) -> Result<(), DatabaseError> {
        sqlx::query(
            "INSERT INTO namespaces (project_id, name) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(project_id)
        .bind(namespace)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Deletes a batch of expired rows; returns how many went. The
    /// sweeper loops this until a sweep comes back empty.
    ///
    /// # Errors
    /// Returns [`DatabaseError`] with postgres's message.
    pub async fn sweep(&self, batch: i64) -> Result<u64, DatabaseError> {
        let result = sqlx::query(
            "DELETE FROM pairs WHERE ctid IN (\
                 SELECT ctid FROM pairs \
                 WHERE expires_at IS NOT NULL AND expires_at <= now() LIMIT $1)",
        )
        .bind(batch)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }
}

#[async_trait::async_trait]
impl KvStore for PostgresStore {
    async fn get(
        &self,
        project_id: &str,
        namespace: &str,
        key: &str,
    ) -> Result<Option<Pair>, DatabaseError> {
        let project_id = Self::project_uuid(project_id)?;

        // Remaining ttl computed in sql, mirroring scylla's TTL(value):
        // seconds left, floor 1, NULL for a durable pair.
        let row = sqlx::query(
            "SELECT \
                 CASE WHEN expires_at IS NULL THEN NULL \
                      ELSE GREATEST(CEIL(EXTRACT(EPOCH FROM (expires_at - now()))), 1)::int4 \
                      END AS ttl, \
                 project_id, namespace, key, value, type \
             FROM pairs \
             WHERE project_id = $1 AND namespace = $2 AND key = $3 \
                 AND (expires_at IS NULL OR expires_at > now())",
        )
        .bind(project_id)
        .bind(namespace)
        .bind(key)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => Ok(Some(Self::row_into_pair(&row)?)),
            None => Ok(None),
        }
    }

    async fn set(&self, pairs: Vec<Pair>) -> Result<(), DatabaseError> {
        for pair in pairs {
            let value_type: String = pair.r#type().into();
            let project_id = Self::project_uuid(&pair.project_id)?;

            sqlx::query(
                "INSERT INTO pairs (project_id, namespace, key, type, value, expires_at) \
                 VALUES ($1, $2, $3, $4, $5, \
                     CASE WHEN $6::int4 > 0 \
                          THEN now() + make_interval(secs => $6::int4) \
                          ELSE NULL END) \
                 ON CONFLICT (project_id, namespace, key) DO UPDATE SET \
                     type = EXCLUDED.type, value = EXCLUDED.value, \
                     expires_at = EXCLUDED.expires_at",
            )
            .bind(project_id)
            .bind(&pair.namespace)
            .bind(&pair.key)
            .bind(value_type)
            .bind(&pair.value)
            .bind(pair.ttl.unwrap_or(0))
            .execute(&self.pool)
            .await?;

            self.register_namespace(project_id, &pair.namespace).await?;
        }

        Ok(())
    }

    async fn delete(&self, pairs: Vec<PairRequest>) -> Result<(), DatabaseError> {
        for pair in pairs {
            let project_id = Self::project_uuid(&pair.project_id)?;

            sqlx::query("DELETE FROM pairs WHERE project_id = $1 AND namespace = $2 AND key = $3")
                .bind(project_id)
                .bind(&pair.namespace)
                .bind(&pair.key)
                .execute(&self.pool)
                .await?;
        }

        Ok(())
    }

    async fn create_namespace(
        &self,
        project_id: &str,
        namespace: &str,
    ) -> Result<(), DatabaseError> {
        let project_id = Self::project_uuid(project_id)?;

        self.register_namespace(project_id, namespace).await?;

        Ok(())
    }

    async fn delete_namespace(
        &self,
        project_id: &str,
        namespace: &str,
    ) -> Result<(), DatabaseError> {
        let project_id = Self::project_uuid(project_id)?;

        sqlx::query("DELETE FROM pairs WHERE project_id = $1 AND namespace = $2")
            .bind(project_id)
            .bind(namespace)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM namespaces WHERE project_id = $1 AND name = $2")
            .bind(project_id)
            .bind(namespace)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    async fn delete_project(&self, project_id: &str) -> Result<(), DatabaseError> {
        let project_id = Self::project_uuid(project_id)?;

        sqlx::query("DELETE FROM pairs WHERE project_id = $1")
            .bind(project_id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM namespaces WHERE project_id = $1")
            .bind(project_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    async fn get_namespaces(
        &self,
        project_id: &str,
    ) -> Result<ListNamespacesResponse, DatabaseError> {
        let project_uuid = Self::project_uuid(project_id)?;

        let names: Vec<String> =
            sqlx::query_scalar("SELECT name FROM namespaces WHERE project_id = $1 ORDER BY name")
                .bind(project_uuid)
                .fetch_all(&self.pool)
                .await?;

        let mut namespaces = vec![];
        for name in names {
            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM pairs \
                 WHERE project_id = $1 AND namespace = $2 \
                     AND (expires_at IS NULL OR expires_at > now())",
            )
            .bind(project_uuid)
            .bind(&name)
            .fetch_one(&self.pool)
            .await?;

            namespaces.push(Namespace {
                project_id: project_id.to_string(),
                name,
                count: count as i32,
            });
        }

        Ok(ListNamespacesResponse { namespaces })
    }

    async fn list(
        &self,
        project_id: &str,
        namespace: &str,
        page_size: i32,
        token: Option<String>,
    ) -> Result<ListPairsResponse, DatabaseError> {
        let project_id = Self::project_uuid(project_id)?;

        if page_size <= 0 {
            return Err(DatabaseError::Invalid(
                "Page size must be positive".to_string(),
            ));
        }

        // The token is the last key of the previous page, base64ed to
        // stay opaque like every backend's token.
        let after = match token {
            None => None,
            Some(v) => {
                let mut output = Vec::new();
                let mut decoder =
                    read::DecoderReader::new(v.as_bytes(), &general_purpose::STANDARD_NO_PAD);
                io::copy(&mut decoder, &mut output)
                    .map_err(|_| DatabaseError::Invalid("Invalid token provided".to_string()))?;
                Some(
                    String::from_utf8(output).map_err(|_| {
                        DatabaseError::Invalid("Invalid token provided".to_string())
                    })?,
                )
            }
        };

        // One row past the page answers "is there more" without a
        // second query.
        let rows = sqlx::query(
            "SELECT \
                 CASE WHEN expires_at IS NULL THEN NULL \
                      ELSE GREATEST(CEIL(EXTRACT(EPOCH FROM (expires_at - now()))), 1)::int4 \
                      END AS ttl, \
                 project_id, namespace, key, value, type \
             FROM pairs \
             WHERE project_id = $1 AND namespace = $2 \
                 AND (expires_at IS NULL OR expires_at > now()) \
                 AND ($3::text IS NULL OR key > $3) \
             ORDER BY key LIMIT $4",
        )
        .bind(project_id)
        .bind(namespace)
        .bind(after)
        .bind(page_size as i64 + 1)
        .fetch_all(&self.pool)
        .await?;

        let more = rows.len() > page_size as usize;
        let mut pairs = vec![];
        for row in rows.iter().take(page_size as usize) {
            pairs.push(Self::row_into_pair(row)?);
        }

        let token = match (more, pairs.last()) {
            (true, Some(last)) => {
                let mut output = String::new();
                write::EncoderStringWriter::from_consumer(
                    &mut output,
                    &general_purpose::STANDARD_NO_PAD,
                )
                .write_all(last.key.as_bytes())
                .map_err(|e| DatabaseError::Invalid(e.to_string()))?;
                Some(output)
            }
            _ => None,
        };

        Ok(ListPairsResponse {
            page_size,
            token,
            pairs,
        })
    }
}

/// Container-backed tests: the shared conformance suite against a real
/// postgres, through the real migrator.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::conformance;
    use serial_test::serial;
    use testcontainers::runners::AsyncRunner;
    use testcontainers::{ContainerAsync, GenericImage, ImageExt, core::WaitFor};

    /// Starts postgres, applies the real migrations, and connects.
    ///
    /// The container rides along because dropping it stops the database.
    async fn store() -> (ContainerAsync<GenericImage>, PostgresStore) {
        let container = GenericImage::new("postgres", "17")
            .with_wait_for(WaitFor::message_on_stderr(
                "database system is ready to accept connections",
            ))
            .with_env_var("POSTGRES_USER", "kv")
            .with_env_var("POSTGRES_PASSWORD", "kv")
            .with_env_var("POSTGRES_DB", "actias_kv")
            .start()
            .await
            .expect("postgres starts");

        let port = container
            .get_host_port_ipv4(5432)
            .await
            .expect("postgres port is published");
        let url = format!("postgresql://kv:kv@127.0.0.1:{port}/actias_kv");

        let pool = connect(&url).await;

        // The real migrator, twice: the second run must find everything
        // recorded and change nothing, or a restarting migration container
        // would corrupt a live deployment.
        crate::migrate::apply_postgres(&pool)
            .await
            .expect("migrations apply");
        crate::migrate::apply_postgres(&pool)
            .await
            .expect("migrations are re-runnable");

        (container, PostgresStore::new(pool))
    }

    #[tokio::test]
    #[serial(containers)]
    async fn pairs_round_trip_and_writes_register_their_namespace() {
        let (_container, db) = store().await;
        conformance::pairs_round_trip_and_writes_register_their_namespace(&db).await;
    }

    #[tokio::test]
    #[serial(containers)]
    async fn a_ttl_write_reports_its_remaining_life() {
        let (_container, db) = store().await;
        conformance::a_ttl_write_reports_its_remaining_life(&db).await;
    }

    #[tokio::test]
    #[serial(containers)]
    async fn listing_pages_one_namespace_and_stops_at_its_end() {
        let (_container, db) = store().await;
        conformance::listing_pages_one_namespace_and_stops_at_its_end(&db).await;
    }

    #[tokio::test]
    #[serial(containers)]
    async fn namespace_and_project_deletion_remove_data_and_registry() {
        let (_container, db) = store().await;
        conformance::namespace_and_project_deletion_remove_data_and_registry(&db).await;
    }

    #[tokio::test]
    #[serial(containers)]
    async fn the_sweeper_reclaims_only_expired_rows() {
        let (_container, db) = store().await;
        let project = uuid::Uuid::new_v4();

        let mut expired = conformance::pair(project, "cache", "old", "gone");
        expired.ttl = Some(1);
        let durable = conformance::pair(project, "cache", "keep", "stays");
        db.set(vec![expired, durable]).await.expect("set succeeds");

        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        let swept = db.sweep(100).await.expect("sweep runs");
        assert_eq!(swept, 1, "exactly the expired row goes");
        assert!(
            db.get(&project.to_string(), "cache", "keep")
                .await
                .expect("get runs")
                .is_some(),
            "the durable row survives the sweep"
        );
    }
}
