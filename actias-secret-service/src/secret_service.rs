//! The four rpcs over `secret_versions`: a write-only management plane
//! (set, delete, list) and the resolution plane workers call. Versions are
//! immutable rows; every mutation is an insert or a tombstone.

use actias_common::tracing::{error, info};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveValue::Set, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, SqlErr,
};
use tonic::{Request, Response, Status};

use crate::entity;
use crate::envelope::{CryptoError, Envelope};
use crate::proto_secret_service::secret_service_server::SecretService as SecretServiceTrait;
use crate::proto_secret_service::{
    DeleteSecretRequest, ListSecretVersionsRequest, ListSecretVersionsResponse, ListSecretsRequest,
    ListSecretsResponse, ResolveSecretRequest, ResolvedSecret, SecretMeta, SecretVersion,
    SetSecretRequest,
};

pub struct SecretService {
    database: DatabaseConnection,
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
fn storage_error(context: &'static str, err: sea_orm::DbErr) -> Status {
    error!("{context}: {err}");
    Status::internal("storage failed")
}

/// Rows of one name in one project, newest first when ordered.
fn by_name(project_id: &str, name: &str) -> sea_orm::Select<entity::Entity> {
    entity::Entity::find()
        .filter(entity::Column::ProjectId.eq(project_id))
        .filter(entity::Column::Name.eq(name))
}

impl SecretService {
    pub fn new(database: DatabaseConnection, envelope: Envelope) -> Self {
        SecretService { database, envelope }
    }

    /// The newest version row of a name, tombstoned or not.
    async fn head(
        &self,
        project_id: &str,
        name: &str,
    ) -> Result<Option<entity::Model>, sea_orm::DbErr> {
        by_name(project_id, name)
            .order_by_desc(entity::Column::Version)
            .one(&self.database)
            .await
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

        let created_ms = unix_now_ms();
        let created_by = (!request.created_by.is_empty()).then(|| request.created_by.clone());

        // The next version is read-then-insert; a concurrent rotation of the
        // same name trips the primary key and simply retries.
        for _ in 0..3 {
            let head = self
                .head(&request.project_id, &request.name)
                .await
                .map_err(|err| storage_error("secret head read failed", err))?;
            let version = head.map_or(0, |row| row.version) + 1;

            // A fresh seal per attempt keeps data keys single-use even
            // across retries.
            let sealed = self
                .envelope
                .seal(request.value.as_bytes())
                .map_err(|err| {
                    error!("sealing failed: {err}");
                    Status::internal("encryption failed")
                })?;

            let inserted = entity::Entity::insert(entity::ActiveModel {
                project_id: Set(request.project_id.clone()),
                name: Set(request.name.clone()),
                version: Set(version),
                kek_id: Set(sealed.kek_id),
                dek_wrapped: Set(sealed.dek_wrapped),
                nonce: Set(sealed.nonce),
                ciphertext: Set(sealed.ciphertext),
                created_ms: Set(created_ms),
                created_by: Set(created_by.clone()),
                deleted_ms: Set(None),
            })
            .exec(&self.database)
            .await;

            match inserted {
                Ok(_) => {
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
                Err(err) => match err.sql_err() {
                    Some(SqlErr::UniqueConstraintViolation(_)) => continue,
                    _ => return Err(storage_error("secret insert failed", err)),
                },
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

        let head = self
            .head(&request.project_id, &request.name)
            .await
            .map_err(|err| storage_error("secret head read failed", err))?;
        let Some(head) = head.filter(|row| row.deleted_ms.is_none()) else {
            return Err(Status::not_found("no secret by that name"));
        };

        // Tombstone exactly the head we read; a rotation racing past it
        // leaves the newer head live, which is the rotation winning.
        let tombstoned = entity::Entity::update_many()
            .col_expr(entity::Column::DeletedMs, Expr::value(unix_now_ms()))
            .filter(entity::Column::ProjectId.eq(&request.project_id))
            .filter(entity::Column::Name.eq(&request.name))
            .filter(entity::Column::Version.eq(head.version))
            .filter(entity::Column::DeletedMs.is_null())
            .exec(&self.database)
            .await
            .map_err(|err| storage_error("secret tombstone failed", err))?;

        if tombstoned.rows_affected == 0 {
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
        let heads = entity::Entity::find()
            .filter(entity::Column::ProjectId.eq(&request.project_id))
            .distinct_on([entity::Column::Name])
            .order_by_asc(entity::Column::Name)
            .order_by_desc(entity::Column::Version)
            .all(&self.database)
            .await
            .map_err(|err| storage_error("secret listing failed", err))?;

        let secrets = heads
            .into_iter()
            .filter(|row| row.deleted_ms.is_none())
            .map(|row| SecretMeta {
                name: row.name,
                version: row.version as u64,
                created_ms: row.created_ms,
                created_by: row.created_by.unwrap_or_default(),
            })
            .collect();

        Ok(Response::new(ListSecretsResponse { secrets }))
    }

    async fn list_secret_versions(
        &self,
        request: Request<ListSecretVersionsRequest>,
    ) -> Result<Response<ListSecretVersionsResponse>, Status> {
        let request = request.into_inner();
        require("project_id", &request.project_id)?;
        require("name", &request.name)?;

        let rows = by_name(&request.project_id, &request.name)
            .order_by_desc(entity::Column::Version)
            .all(&self.database)
            .await
            .map_err(|err| storage_error("secret history read failed", err))?;

        let versions = rows
            .into_iter()
            .map(|row| SecretVersion {
                version: row.version as u64,
                created_ms: row.created_ms,
                created_by: row.created_by.unwrap_or_default(),
                deleted_ms: row.deleted_ms.unwrap_or(0),
            })
            .collect();

        Ok(Response::new(ListSecretVersionsResponse { versions }))
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
            self.head(&request.project_id, &request.name)
                .await
                .map_err(|err| storage_error("secret head read failed", err))?
                .filter(|row| row.deleted_ms.is_none())
        } else {
            // An exact pin: resolvable regardless of tombstones, because a
            // workflow run finishes with the credentials it started with.
            entity::Entity::find_by_id((
                request.project_id.clone(),
                request.name.clone(),
                request.version as i64,
            ))
            .one(&self.database)
            .await
            .map_err(|err| storage_error("secret version read failed", err))?
        };

        let Some(row) = row else {
            return Err(Status::not_found("no secret by that name"));
        };

        let plaintext = self
            .envelope
            .open(&row.kek_id, &row.dek_wrapped, &row.nonce, &row.ciphertext)
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

        info!(
            "secret resolved: project {} name {} version {} script {}",
            request.project_id,
            request.name,
            row.version,
            if request.script_id.is_empty() {
                "-"
            } else {
                &request.script_id
            },
        );

        Ok(Response::new(ResolvedSecret {
            value,
            version: row.version as u64,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::KEY_LEN;
    use sea_orm::{ConnectionTrait, Database};
    use testcontainers_modules::{
        postgres::Postgres as PostgresImage,
        testcontainers::{ImageExt, runners::AsyncRunner},
    };
    use zeroize::Zeroizing;

    /// A service over a real postgres with the migration applied.
    async fn service() -> (
        SecretService,
        DatabaseConnection,
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

        let database = Database::connect(format!(
            "postgresql://postgres:postgres@127.0.0.1:{port}/postgres"
        ))
        .await
        .expect("postgres accepts connections");

        // The same file the migration container applies.
        database
            .execute_unprepared(include_str!(
                "../migrations/20260821003926_secret_versions.up.sql"
            ))
            .await
            .expect("migration applies");

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
    async fn history_lists_every_version_newest_first_with_tombstones() {
        let (service, _database, _guard) = service().await;

        set(&service, "stripe-live", "v1").await;
        set(&service, "stripe-live", "v2").await;
        delete(&service, "stripe-live").await.expect("deletes");
        set(&service, "stripe-live", "v3").await;

        let versions = service
            .list_secret_versions(Request::new(ListSecretVersionsRequest {
                project_id: "proj-1".to_owned(),
                name: "stripe-live".to_owned(),
            }))
            .await
            .expect("history lists")
            .into_inner()
            .versions;

        assert_eq!(
            versions.iter().map(|v| v.version).collect::<Vec<_>>(),
            vec![3, 2, 1],
        );
        // Only the deleted head carries a tombstone; the revival is live.
        assert_eq!(
            versions
                .iter()
                .map(|v| v.deleted_ms > 0)
                .collect::<Vec<_>>(),
            vec![false, true, false],
        );
        assert!(versions.iter().all(|v| v.created_by == "user-1"));
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
        let row =
            entity::Entity::find_by_id(("proj-1".to_owned(), "stripe-live".to_owned(), 1_i64))
                .one(&database)
                .await
                .expect("row reads")
                .expect("row exists");
        let (kek_id, rewrapped) = rotated
            .rewrap(&row.kek_id, &row.dek_wrapped)
            .expect("rewraps");
        entity::Entity::update_many()
            .col_expr(entity::Column::KekId, Expr::value(kek_id))
            .col_expr(entity::Column::DekWrapped, Expr::value(rewrapped))
            .filter(entity::Column::ProjectId.eq("proj-1"))
            .filter(entity::Column::Name.eq("stripe-live"))
            .filter(entity::Column::Version.eq(1))
            .exec(&database)
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
