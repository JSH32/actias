//! The four rpcs over `secret_versions`: a write-only management plane
//! (set, delete, list) and the resolution plane workers call. Versions are
//! immutable rows; every mutation is an insert or a tombstone.

use actias_common::tracing::{error, info};
use sqlx::{Pool, Postgres, Row};
use tonic::{Request, Response, Status};

use crate::envelope::{CryptoError, Envelope};
use crate::proto_secret_service::secret_service_server::SecretService as SecretServiceTrait;
use crate::proto_secret_service::{
    DeleteSecretRequest, ListSecretsRequest, ListSecretsResponse, ResolveSecretRequest,
    ResolvedSecret, SecretMeta, SetSecretRequest,
};

pub struct SecretService {
    database: Pool<Postgres>,
    envelope: Envelope,
}

fn unix_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

/// Validates one identifier-ish request field without echoing internals.
#[allow(clippy::result_large_err)]
fn require(field: &'static str, value: &str) -> Result<(), Status> {
    if value.is_empty() {
        return Err(Status::invalid_argument(format!("{field} is required")));
    }
    if value.len() > 256 || value.chars().any(char::is_control) {
        return Err(Status::invalid_argument(format!(
            "{field} is not a valid name"
        )));
    }
    Ok(())
}

/// One storage failure surface: the caller hears "storage failed", the log
/// hears why.
fn storage_error(context: &'static str, err: sqlx::Error) -> Status {
    error!("{context}: {err}");
    Status::internal("storage failed")
}

impl SecretService {
    pub fn new(database: Pool<Postgres>, envelope: Envelope) -> Self {
        SecretService { database, envelope }
    }
}

#[tonic::async_trait]
impl SecretServiceTrait for SecretService {
    async fn set_secret(
        &self,
        request: Request<SetSecretRequest>,
    ) -> Result<Response<SecretMeta>, Status> {
        let request = request.into_inner();
        require("project_id", &request.project_id)?;
        require("name", &request.name)?;

        let sealed = self
            .envelope
            .seal(request.value.as_bytes())
            .map_err(|err| {
                error!("sealing failed: {err}");
                Status::internal("encryption failed")
            })?;
        let created_ms = unix_now_ms();
        let created_by = (!request.created_by.is_empty()).then_some(&request.created_by);

        // The next version is read-then-insert; a concurrent rotation of the
        // same name trips the primary key and simply retries.
        for _ in 0..3 {
            let inserted = sqlx::query(
                "INSERT INTO secret_versions \
                     (project_id, name, version, kek_id, dek_wrapped, nonce, ciphertext, \
                      created_ms, created_by) \
                 VALUES ($1, $2, \
                     (SELECT COALESCE(MAX(version), 0) + 1 FROM secret_versions \
                      WHERE project_id = $1 AND name = $2), \
                     $3, $4, $5, $6, $7, $8) \
                 RETURNING version",
            )
            .bind(&request.project_id)
            .bind(&request.name)
            .bind(&sealed.kek_id)
            .bind(&sealed.dek_wrapped)
            .bind(&sealed.nonce)
            .bind(&sealed.ciphertext)
            .bind(created_ms)
            .bind(created_by)
            .fetch_one(&self.database)
            .await;

            match inserted {
                Ok(row) => {
                    let version: i64 = row.get("version");
                    info!(
                        "secret set: project {} name {} version {version}",
                        request.project_id, request.name
                    );
                    return Ok(Response::new(SecretMeta {
                        name: request.name,
                        version: version as u64,
                        created_ms,
                        created_by: request.created_by,
                    }));
                }
                Err(sqlx::Error::Database(db)) if db.is_unique_violation() => continue,
                Err(err) => return Err(storage_error("secret insert failed", err)),
            }
        }

        Err(Status::aborted("concurrent rotation, retry"))
    }

    async fn delete_secret(
        &self,
        request: Request<DeleteSecretRequest>,
    ) -> Result<Response<()>, Status> {
        let request = request.into_inner();
        require("project_id", &request.project_id)?;
        require("name", &request.name)?;

        let tombstoned = sqlx::query(
            "UPDATE secret_versions SET deleted_ms = $3 \
             WHERE project_id = $1 AND name = $2 AND deleted_ms IS NULL \
               AND version = (SELECT MAX(version) FROM secret_versions \
                              WHERE project_id = $1 AND name = $2)",
        )
        .bind(&request.project_id)
        .bind(&request.name)
        .bind(unix_now_ms())
        .execute(&self.database)
        .await
        .map_err(|err| storage_error("secret tombstone failed", err))?;

        if tombstoned.rows_affected() == 0 {
            return Err(Status::not_found("no secret by that name"));
        }

        info!(
            "secret deleted: project {} name {}",
            request.project_id, request.name
        );
        Ok(Response::new(()))
    }

    async fn list_secrets(
        &self,
        request: Request<ListSecretsRequest>,
    ) -> Result<Response<ListSecretsResponse>, Status> {
        let request = request.into_inner();
        require("project_id", &request.project_id)?;

        // Head row per name, then live heads only: a tombstoned head hides
        // the name even though its older versions remain resolvable by pin.
        let rows = sqlx::query(
            "SELECT name, version, created_ms, created_by FROM ( \
                 SELECT DISTINCT ON (name) name, version, created_ms, created_by, deleted_ms \
                 FROM secret_versions WHERE project_id = $1 \
                 ORDER BY name, version DESC \
             ) heads WHERE deleted_ms IS NULL ORDER BY name",
        )
        .bind(&request.project_id)
        .fetch_all(&self.database)
        .await
        .map_err(|err| storage_error("secret listing failed", err))?;

        let secrets = rows
            .into_iter()
            .map(|row| SecretMeta {
                name: row.get("name"),
                version: row.get::<i64, _>("version") as u64,
                created_ms: row.get("created_ms"),
                created_by: row
                    .get::<Option<String>, _>("created_by")
                    .unwrap_or_default(),
            })
            .collect();

        Ok(Response::new(ListSecretsResponse { secrets }))
    }

    async fn resolve_secret(
        &self,
        request: Request<ResolveSecretRequest>,
    ) -> Result<Response<ResolvedSecret>, Status> {
        let request = request.into_inner();
        require("project_id", &request.project_id)?;
        require("name", &request.name)?;

        let row = if request.version == 0 {
            // The head: refused when tombstoned, so a deleted name stops
            // resolving even though its rows remain.
            sqlx::query(
                "SELECT version, kek_id, dek_wrapped, nonce, ciphertext, deleted_ms \
                 FROM secret_versions WHERE project_id = $1 AND name = $2 \
                 ORDER BY version DESC LIMIT 1",
            )
            .bind(&request.project_id)
            .bind(&request.name)
            .fetch_optional(&self.database)
            .await
            .map_err(|err| storage_error("secret head read failed", err))?
            .filter(|row| row.get::<Option<i64>, _>("deleted_ms").is_none())
        } else {
            // An exact pin: resolvable regardless of tombstones, because a
            // workflow run finishes with the credentials it started with.
            sqlx::query(
                "SELECT version, kek_id, dek_wrapped, nonce, ciphertext, deleted_ms \
                 FROM secret_versions \
                 WHERE project_id = $1 AND name = $2 AND version = $3",
            )
            .bind(&request.project_id)
            .bind(&request.name)
            .bind(request.version as i64)
            .fetch_optional(&self.database)
            .await
            .map_err(|err| storage_error("secret version read failed", err))?
        };

        let Some(row) = row else {
            return Err(Status::not_found("no secret by that name"));
        };

        let kek_id: String = row.get("kek_id");
        let plaintext = self
            .envelope
            .open(
                &kek_id,
                &row.get::<Vec<u8>, _>("dek_wrapped"),
                &row.get::<Vec<u8>, _>("nonce"),
                &row.get::<Vec<u8>, _>("ciphertext"),
            )
            .map_err(|err| {
                match err {
                    CryptoError::UnknownKek(id) => error!(
                        "secret wrapped by unheld kek '{id}': project {} name {}",
                        request.project_id, request.name
                    ),
                    CryptoError::Corrupt => error!(
                        "secret failed to open: project {} name {}",
                        request.project_id, request.name
                    ),
                }
                Status::internal("decryption failed")
            })?;

        let value = String::from_utf8(plaintext.to_vec()).map_err(|_| {
            error!(
                "secret is not utf-8: project {} name {}",
                request.project_id, request.name
            );
            Status::internal("decryption failed")
        })?;

        let version: i64 = row.get("version");
        info!(
            "secret resolved: project {} name {} version {version} script {}",
            request.project_id,
            request.name,
            if request.script_id.is_empty() {
                "-"
            } else {
                &request.script_id
            },
        );

        Ok(Response::new(ResolvedSecret {
            value,
            version: version as u64,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::KEY_LEN;
    use sqlx::postgres::PgPoolOptions;
    use testcontainers_modules::{
        postgres::Postgres as PostgresImage,
        testcontainers::{ImageExt, runners::AsyncRunner},
    };
    use zeroize::Zeroizing;

    /// A service over a real postgres with the migrations applied.
    async fn service() -> (
        SecretService,
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

        let envelope = Envelope::new("kek-1".to_owned(), Zeroizing::new([7u8; KEY_LEN]), None);
        (
            SecretService::new(database.clone(), envelope),
            database,
            postgres,
        )
    }

    async fn set(service: &SecretService, name: &str, value: &str) -> SecretMeta {
        service
            .set_secret(Request::new(SetSecretRequest {
                project_id: "proj-1".to_owned(),
                name: name.to_owned(),
                value: value.to_owned(),
                created_by: "user-1".to_owned(),
            }))
            .await
            .expect("secret sets")
            .into_inner()
    }

    async fn resolve(
        service: &SecretService,
        name: &str,
        version: u64,
    ) -> Result<ResolvedSecret, Status> {
        service
            .resolve_secret(Request::new(ResolveSecretRequest {
                project_id: "proj-1".to_owned(),
                name: name.to_owned(),
                version,
                script_id: String::new(),
            }))
            .await
            .map(|response| response.into_inner())
    }

    async fn list(service: &SecretService) -> Vec<SecretMeta> {
        service
            .list_secrets(Request::new(ListSecretsRequest {
                project_id: "proj-1".to_owned(),
            }))
            .await
            .expect("secrets list")
            .into_inner()
            .secrets
    }

    async fn delete(service: &SecretService, name: &str) -> Result<(), Status> {
        service
            .delete_secret(Request::new(DeleteSecretRequest {
                project_id: "proj-1".to_owned(),
                name: name.to_owned(),
            }))
            .await
            .map(|_| ())
    }

    #[tokio::test]
    async fn a_set_secret_resolves_at_the_head() {
        let (service, _database, _guard) = service().await;

        let meta = set(&service, "stripe-live", "sk_live_51H8xQ2").await;
        assert_eq!(meta.version, 1);

        let resolved = resolve(&service, "stripe-live", 0).await.expect("resolves");
        assert_eq!(resolved.value, "sk_live_51H8xQ2");
        assert_eq!(resolved.version, 1);
    }

    #[tokio::test]
    async fn rotation_moves_the_head_and_keeps_old_versions_pinnable() {
        let (service, _database, _guard) = service().await;

        set(&service, "stripe-live", "old value").await;
        let rotated = set(&service, "stripe-live", "new value").await;
        assert_eq!(rotated.version, 2);

        let head = resolve(&service, "stripe-live", 0)
            .await
            .expect("head resolves");
        assert_eq!((head.value.as_str(), head.version), ("new value", 2));

        // A workflow that pinned version 1 still gets what it started with.
        let pinned = resolve(&service, "stripe-live", 1)
            .await
            .expect("pin resolves");
        assert_eq!(pinned.value, "old value");
    }

    #[tokio::test]
    async fn delete_hides_the_name_while_pins_keep_resolving() {
        let (service, _database, _guard) = service().await;

        set(&service, "stripe-live", "secret value").await;
        delete(&service, "stripe-live").await.expect("deletes");

        assert!(list(&service).await.is_empty());
        let head = resolve(&service, "stripe-live", 0).await;
        assert_eq!(
            head.expect_err("head is gone").code(),
            tonic::Code::NotFound
        );

        let pinned = resolve(&service, "stripe-live", 1)
            .await
            .expect("pin resolves");
        assert_eq!(pinned.value, "secret value");

        // Deleting again refuses: there is no live head to tombstone.
        let again = delete(&service, "stripe-live").await;
        assert_eq!(
            again.expect_err("nothing to delete").code(),
            tonic::Code::NotFound
        );

        // Setting the name again continues the version sequence live.
        let revived = set(&service, "stripe-live", "fresh value").await;
        assert_eq!(revived.version, 2);
        let head = resolve(&service, "stripe-live", 0)
            .await
            .expect("head is back");
        assert_eq!(head.value, "fresh value");
    }

    #[tokio::test]
    async fn listing_shows_live_names_only_with_metadata() {
        let (service, _database, _guard) = service().await;

        set(&service, "stripe-live", "a").await;
        set(&service, "sendgrid-key", "b").await;
        set(&service, "sendgrid-key", "b2").await;
        delete(&service, "stripe-live").await.expect("deletes");

        let secrets = list(&service).await;
        assert_eq!(secrets.len(), 1);
        assert_eq!(secrets[0].name, "sendgrid-key");
        assert_eq!(secrets[0].version, 2);
        assert_eq!(secrets[0].created_by, "user-1");
        assert!(secrets[0].created_ms > 0);
    }

    #[tokio::test]
    async fn unknown_names_and_versions_are_not_found() {
        let (service, _database, _guard) = service().await;
        set(&service, "stripe-live", "value").await;

        let missing = resolve(&service, "no-such-name", 0).await;
        assert_eq!(
            missing.expect_err("unknown name").code(),
            tonic::Code::NotFound
        );

        let missing = resolve(&service, "stripe-live", 7).await;
        assert_eq!(
            missing.expect_err("unknown version").code(),
            tonic::Code::NotFound
        );
    }

    #[tokio::test]
    async fn projects_do_not_see_each_others_secrets() {
        let (service, _database, _guard) = service().await;
        set(&service, "stripe-live", "value").await;

        let other = service
            .resolve_secret(Request::new(ResolveSecretRequest {
                project_id: "proj-2".to_owned(),
                name: "stripe-live".to_owned(),
                version: 0,
                script_id: String::new(),
            }))
            .await;
        assert_eq!(
            other.expect_err("other project sees nothing").code(),
            tonic::Code::NotFound
        );
    }

    #[tokio::test]
    async fn a_rewrapped_row_resolves_under_the_new_master() {
        let (service, database, _guard) = service().await;
        set(&service, "stripe-live", "survives rotation").await;

        // The rotated process: kek-2 active, kek-1 readable; it re-wraps
        // the row and forgets the old master entirely.
        let rotated = Envelope::new(
            "kek-2".to_owned(),
            Zeroizing::new([9u8; KEY_LEN]),
            Some(("kek-1".to_owned(), Zeroizing::new([7u8; KEY_LEN]))),
        );
        let row = sqlx::query(
            "SELECT kek_id, dek_wrapped FROM secret_versions \
             WHERE project_id = 'proj-1' AND name = 'stripe-live' AND version = 1",
        )
        .fetch_one(&database)
        .await
        .expect("row reads");
        let (kek_id, rewrapped) = rotated
            .rewrap(
                &row.get::<String, _>("kek_id"),
                &row.get::<Vec<u8>, _>("dek_wrapped"),
            )
            .expect("rewraps");
        sqlx::query(
            "UPDATE secret_versions SET kek_id = $1, dek_wrapped = $2 \
             WHERE project_id = 'proj-1' AND name = 'stripe-live' AND version = 1",
        )
        .bind(&kek_id)
        .bind(&rewrapped)
        .execute(&database)
        .await
        .expect("rewrap lands");

        let after = SecretService::new(
            database.clone(),
            Envelope::new("kek-2".to_owned(), Zeroizing::new([9u8; KEY_LEN]), None),
        );
        let resolved = resolve(&after, "stripe-live", 0).await.expect("resolves");
        assert_eq!(resolved.value, "survives rotation");
    }
}
