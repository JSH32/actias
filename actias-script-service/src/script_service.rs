use std::str::FromStr;

use actias_common::logging::{live_log_channel, script_log_channel};
use futures::StreamExt;
use futures::future::join_all;
use sqlx::types::Uuid;
use sqlx::{Pool, Postgres};
use tonic::{Response, Status};

use crate::blob_store::BlobStore;
use crate::bundle::{Bundle, File};
use crate::database_types::{DbFile, DbRevision, DbScript, ScriptConfig};
use crate::live_script::LiveScriptManager;
use crate::proto_script_service::find_script_request::{self};
use crate::proto_script_service::{
    ListRevisionResponse, ListScriptResponse, Revision, Script, script_service_server, *,
};

use crate::proto_script_service::find_script_request::Query::{Id, PublicName};
use crate::util::safe_divide;

/// # TODOs
/// - Split database into it's own module.
/// - Switch to MySQL for scalability (TiDB).
/// - Explore light ORM's like [rbatis](https://github.com/rbatis/rbatis).
pub struct ScriptService {
    database: Pool<Postgres>,
    live_script_manager: LiveScriptManager,
    blobs: BlobStore,
}

impl ScriptService {
    pub fn new(
        database: Pool<Postgres>,
        live_script_manager: LiveScriptManager,
        blobs: BlobStore,
    ) -> Self {
        Self {
            database,
            live_script_manager,
            blobs,
        }
    }

    async fn get_script_info(
        &self,
        script_query: find_script_request::Query,
    ) -> Result<DbScript, tonic::Status> {
        let sql = &format!(
            "SELECT * FROM scripts WHERE {} = $1",
            match &script_query {
                Id(_) => "id",
                PublicName(_) => "public_identifier",
            }
        );

        let qb = sqlx::query_as::<_, DbScript>(sql);

        let query = match &script_query {
            Id(v) => qb.bind(
                Uuid::parse_str(v)
                    .map_err(|_| Status::invalid_argument("'id' was not a valid uuid"))?,
            ),
            PublicName(v) => qb.bind(v.clone()),
        };

        query
            .fetch_optional(&self.database)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or(Status::not_found(format!(
                "Script with that {} was not found",
                match &script_query {
                    Id(_) => "id",
                    PublicName(_) => "identifier",
                }
            )))
    }

    async fn get_db_revision(&self, revision_id: &str) -> Result<DbRevision, tonic::Status> {
        sqlx::query_as::<_, DbRevision>("SELECT * FROM revisions WHERE id = $1")
            .bind(
                Uuid::parse_str(revision_id)
                    .map_err(|_| Status::invalid_argument("'id' was not a valid uuid"))?,
            )
            .fetch_optional(&self.database)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or(Status::not_found("Revision with that ID was not found"))
    }

    async fn create_db_revision(
        &self,
        script_id: &Uuid,
        mut script_config: ScriptConfig,
        mut bundle: Bundle,
    ) -> Result<Revision, tonic::Status> {
        if &script_config.id != script_id {
            return Err(Status::invalid_argument(
                "Script config contains a different ID than the target.",
            ));
        }

        // The declaration pass runs over the code as it will execute, so the
        // stored contract is derived here, never taken from the client.
        // Manifest-only lua files are read back from the blob store first.
        let mut sources: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for file in bundle.files.iter() {
            if !file.file_path.ends_with(".lua") {
                continue;
            }

            let bytes = if file.hash.is_empty() || !file.content.is_empty() {
                file.content.clone()
            } else {
                self.blobs.get(&file.hash).await?
            };

            if let Ok(source) = String::from_utf8(bytes) {
                sources.insert(file.file_path.clone(), source);
            }
        }

        let entry_point = bundle.entry_point.clone();
        let derived = tokio::task::spawn_blocking(move || {
            actias_declarations::extract(sources, &entry_point)
        })
        .await
        .map_err(|e| Status::internal(e.to_string()))?
        .map_err(Status::invalid_argument)?;

        script_config.capabilities = Some(crate::database_types::Capabilities {
            kv: derived.kv,
            events: derived.events,
            secrets: derived.secrets,
            objects: derived.objects,
        });

        // Files arrive either inline (content present, hashed and stored
        // here so the hash is authoritative) or manifest-only (a claimed
        // hash whose blob must already be stored). An empty file with no
        // claimed hash is still inline: empty content hashes fine.
        for file in bundle.files.iter_mut() {
            if file.hash.is_empty() || !file.content.is_empty() {
                file.hash = blake3::hash(&file.content).to_hex().to_string();
                file.size = file.content.len() as u64;
                self.blobs
                    .put(&file.hash, std::mem::take(&mut file.content))
                    .await?;
            } else {
                let Some(size) = self.blobs.head(&file.hash).await? else {
                    return Err(Status::failed_precondition(format!(
                        "Blob for '{}' is not stored; upload it first.",
                        file.file_path
                    )));
                };
                file.size = size;
            }
        }

        let mut tx = self
            .database
            .begin()
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let revision_info = sqlx::query_as::<_, DbRevision>(
            "INSERT INTO revisions (script_id, entry_point, script_config) VALUES ($1, $2, $3) RETURNING *",
        )
        .bind(script_id)
        .bind(bundle.entry_point)
        .bind(sqlx::types::Json(script_config))
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| Status::internal(e.to_string()))?;

        for file in bundle.files.iter() {
            sqlx::query(
                "INSERT INTO files (revision_id, file_path, hash, size, content_type, kind) \
                 VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(revision_info.id)
            .bind(&file.file_path)
            .bind(&file.hash)
            .bind(file.size as i64)
            .bind(&file.content_type)
            .bind(file.kind as i16)
            .execute(&mut *tx)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        }

        sqlx::query("UPDATE scripts SET current_revision = $1, last_updated = now() WHERE id = $2")
            .bind(revision_info.id)
            .bind(script_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        tx.commit()
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Revision {
            id: revision_info.id.to_string(),
            created: revision_info.created.to_string(),
            script_id: revision_info.script_id.to_string(),
            bundle: None,
            script_config: Some(revision_info.script_config.0.into()),
        })
    }
}

#[tonic::async_trait]
impl script_service_server::ScriptService for ScriptService {
    async fn delete_project(
        &self,
        request: tonic::Request<DeleteProjectRequest>,
    ) -> Result<tonic::Response<()>, tonic::Status> {
        let request = request.get_ref();

        let project_id =
            Uuid::from_str(&request.project_id).map_err(|e| Status::internal(e.to_string()))?;

        let mut tx = self
            .database
            .begin()
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Get these to delete from live script redis.
        let script_ids =
            sqlx::query_as::<_, (String,)>("SELECT id::text FROM scripts WHERE project_id = $1")
                .bind(project_id)
                .fetch_all(&mut *tx)
                .await
                .map_err(|e| Status::internal(e.to_string()))?;

        sqlx::query("DELETE FROM scripts WHERE project_id = $1")
            .bind(project_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        tx.commit()
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // We run all the delete queries at once and just make sure they all complete.
        let _ = join_all(
            script_ids
                .iter()
                .map(|script_id| self.live_script_manager.delete_script(&script_id.0)),
        )
        .await;

        Ok(Response::new(()))
    }

    async fn create_revision(
        &self,
        request: tonic::Request<CreateRevisionRequest>,
    ) -> Result<tonic::Response<Revision>, tonic::Status> {
        let request = request.get_ref().clone();

        let script_info = self
            .get_script_info(find_script_request::Query::Id(request.script_id.clone()))
            .await?;

        Ok(Response::new(
            self.create_db_revision(
                &script_info.id,
                request
                    .script_config
                    .ok_or_else(|| Status::invalid_argument("script_config is required"))?
                    .try_into()
                    .map_err(|e: uuid::Error| Status::invalid_argument(e.to_string()))?,
                request
                    .bundle
                    .ok_or_else(|| Status::invalid_argument("bundle is required"))?,
            )
            .await
            .map_err(|e| Status::internal(e.to_string()))?,
        ))
    }

    async fn get_revision(
        &self,
        request: tonic::Request<GetRevisionRequest>,
    ) -> Result<tonic::Response<Revision>, tonic::Status> {
        let request = request.get_ref();
        let id = Uuid::from_str(&request.id).map_err(|e| Status::internal(e.to_string()))?;

        let revision_info =
            sqlx::query_as::<_, DbRevision>("SELECT * FROM revisions WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.database)
                .await
                .map_err(|e| Status::internal(e.to_string()))?
                .ok_or(Status::not_found("Revision with that ID was not found"))?;

        let mut bundle: Option<Bundle> = None;
        if request.with_bundle {
            let files = sqlx::query_as::<_, DbFile>(
                r#"
                SELECT f.file_path, f.hash, f.size, f.content_type, f.kind
                FROM files f, revisions r
                WHERE revision_id = $1 AND r.id = f.revision_id
                "#,
            )
            .bind(revision_info.id)
            .fetch_all(&self.database)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

            let mut bundle_files = Vec::with_capacity(files.len());
            for file in &files {
                let hash = file.hash.trim().to_string();

                // A manifest-only caller holds blob store access and pulls
                // the bytes itself; hydrating here would move every bundle
                // through this service twice.
                let content = if request.manifest_only {
                    Vec::new()
                } else {
                    self.blobs.get(&hash).await?
                };

                bundle_files.push(File {
                    content,
                    file_path: file.file_path.clone(),
                    hash,
                    size: file.size as u64,
                    content_type: file.content_type.clone(),
                    kind: file.kind as i32,
                });
            }

            bundle = Some(Bundle {
                entry_point: revision_info.entry_point,
                files: bundle_files,
            })
        };

        Ok(Response::new(Revision {
            id: revision_info.id.to_string(),
            created: revision_info.created.to_string(),
            script_id: revision_info.script_id.to_string(),
            script_config: Some(revision_info.script_config.0.into()),
            bundle,
        }))
    }

    async fn list_revisions(
        &self,
        request: tonic::Request<ListRevisionsRequest>,
    ) -> Result<tonic::Response<ListRevisionResponse>, tonic::Status> {
        let request = request.get_ref();

        if request.page < 1 {
            return Err(Status::invalid_argument("invalid page number provided!"));
        }

        let mut count_query = sqlx::query_as(if request.script_id.is_some() {
            "SELECT COUNT(*) as count FROM revisions WHERE script_id = $1"
        } else {
            "SELECT COUNT(*) as count FROM revisions"
        });

        let mut query = sqlx::query_as::<_, DbRevision>(if request.script_id.is_some() {
            "SELECT * FROM revisions WHERE script_id = $3 ORDER BY created DESC LIMIT $1 OFFSET $2"
        } else {
            "SELECT * FROM revisions ORDER BY created DESC LIMIT $1 OFFSET $2"
        })
        .bind(request.page_size)
        .bind(request.page_size * (request.page - 1));

        if let Some(script_id) = &request.script_id {
            let uuid = Uuid::from_str(script_id).map_err(|e| Status::internal(e.to_string()))?;

            count_query = count_query.bind(uuid);
            query = query.bind(uuid)
        }

        let count: (i64,) = count_query
            .fetch_one(&self.database)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(ListRevisionResponse {
            page: request.page,
            total_pages: safe_divide!(count.0 as i32, request.page_size),
            revisions: query
                .fetch_all(&self.database)
                .await
                .map_err(|e| Status::internal(e.to_string()))?
                .iter()
                .map(|r| Revision {
                    id: r.id.to_string(),
                    created: r.created.to_string(),
                    script_id: r.script_id.to_string(),
                    script_config: Some(r.script_config.clone().0.into()),
                    bundle: None,
                })
                .collect(),
        }))
    }

    async fn delete_revision(
        &self,
        request: tonic::Request<DeleteRevisionRequest>,
    ) -> Result<tonic::Response<NewRevisionResponse>, tonic::Status> {
        let request = request.get_ref();

        let revision = self.get_db_revision(&request.revision_id).await?;
        let script = self
            .get_script_info(find_script_request::Query::Id(
                revision.script_id.to_string(),
            ))
            .await?;

        let mut tx = self
            .database
            .begin()
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        sqlx::query("DELETE FROM revisions WHERE id = $1")
            .bind(revision.id)
            .execute(&mut *tx)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let newest_revision = sqlx::query_as::<_, DbRevision>(
            "SELECT * FROM revisions WHERE script_id = $1 ORDER BY created LIMIT 1",
        )
        .bind(script.id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| Status::internal(e.to_string()))?;

        let current_revision: (Option<Uuid>,) = sqlx::query_as(
            "UPDATE scripts SET current_revision = $1, last_updated = now() WHERE id = $2 RETURNING current_revision",
        )
        .bind(match newest_revision {
            Some(v) => Some(v.id),
            None => None,
        }).bind(script.id).fetch_one(&mut *tx).await.map_err(|e| Status::internal(e.to_string()))?;

        tx.commit()
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(NewRevisionResponse {
            script_id: script.id.to_string(),
            revision_id: current_revision.0.map(|u| u.to_string()),
        }))
    }

    async fn set_script_revision(
        &self,
        request: tonic::Request<SetRevisionRequest>,
    ) -> Result<tonic::Response<NewRevisionResponse>, tonic::Status> {
        let request = request.get_ref();

        let script = self
            .get_script_info(find_script_request::Query::Id(
                request.script_id.to_string(),
            ))
            .await?;
        let revision = self.get_db_revision(&request.revision_id).await?;

        if revision.script_id != script.id {
            return Err(Status::invalid_argument(format!(
                "Script ({}) doesn't own the revision ({})",
                script.id, revision.id
            )));
        }

        sqlx::query("UPDATE scripts SET current_revision = $1, last_updated = now() WHERE id = $2")
            .bind(revision.id)
            .bind(script.id)
            .execute(&self.database)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(NewRevisionResponse {
            script_id: script.id.to_string(),
            revision_id: Some(revision.id.to_string()),
        }))
    }

    async fn list_scripts(
        &self,
        request: tonic::Request<ListScriptRequest>,
    ) -> Result<tonic::Response<ListScriptResponse>, tonic::Status> {
        let request = request.get_ref();

        if request.page < 1 {
            return Err(Status::invalid_argument("Invalid page number provided!"));
        }

        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) as count FROM scripts")
            .fetch_one(&self.database)
            .await
            .map_err(|e: sqlx::Error| Status::internal(e.to_string()))?;

        let project_id = Uuid::from_str(&request.project_id)
            .map_err(|e| Status::invalid_argument(e.to_string()))?;

        Ok(Response::new(ListScriptResponse {
            page: request.page,
            total_pages: safe_divide!(count.0 as i32, request.page_size),
            scripts: sqlx::query_as::<_, DbScript>(
                "SELECT * FROM scripts WHERE project_id = $1 ORDER BY last_updated DESC LIMIT $2 OFFSET $3",
            )
            .bind(project_id)
            .bind(request.page_size)
            .bind(request.page_size * (request.page - 1))
            .fetch_all(&self.database)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .iter()
            .map(|s| Into::<Script>::into((*s).clone()))
            .collect(),
        }))
    }

    async fn create_script(
        &self,
        request: tonic::Request<CreateScriptRequest>,
    ) -> Result<tonic::Response<Script>, tonic::Status> {
        let request = request.get_ref().clone();

        let project_id = Uuid::from_str(&request.project_id)
            .map_err(|e| Status::invalid_argument(e.to_string()))?;

        let script_info = match self
            .get_script_info(find_script_request::Query::PublicName(
                request.public_identifier.clone(),
            ))
            .await
        {
            Ok(_) => {
                return Err(Status::already_exists(
                    "Script with that identifier already exists",
                ));
            }
            Err(e) => match e.code() {
                tonic::Code::NotFound => {
                    // Create a script.
                    sqlx::query_as::<_, DbScript>(
                        "INSERT INTO scripts (public_identifier, project_id) VALUES ($1, $2) RETURNING *",
                    )
                    .bind(request.public_identifier)
                    .bind(project_id)
                    .fetch_one(&self.database)
                    .await
                    .map_err(|e| Status::internal(e.to_string()))?
                }
                _ => return Err(e),
            },
        };

        Ok(Response::new(script_info.into()))
    }

    async fn delete_script(
        &self,
        request: tonic::Request<DeleteScriptRequest>,
    ) -> Result<tonic::Response<()>, tonic::Status> {
        let script_id = &request.get_ref().script_id;

        let row: Option<(Uuid,)> = sqlx::query_as("DELETE FROM scripts WHERE id = $1 RETURNING id")
            .bind(Uuid::from_str(script_id).map_err(|e| Status::internal(e.to_string()))?)
            .fetch_optional(&self.database)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let _ = self.live_script_manager.delete_script(script_id).await;

        match row {
            // Empty response means success
            Some(_) => Ok(Response::new(())),
            None => Err(Status::not_found("Script was not found.")),
        }
    }

    async fn query_script(
        &self,
        request: tonic::Request<FindScriptRequest>,
    ) -> Result<tonic::Response<Script>, tonic::Status> {
        let request = request.get_ref().clone();

        Ok(Response::new(
            self.get_script_info(request.query.unwrap()).await?.into(),
        ))
    }

    async fn put_live_session(
        &self,
        request: tonic::Request<LiveScript>,
    ) -> Result<tonic::Response<LiveScriptSession>, tonic::Status> {
        let request = request.get_ref();
        let session_id = self
            .live_script_manager
            .put_session(request.clone())
            .await?;

        Ok(Response::new(LiveScriptSession {
            script_id: request.script_id.clone(),
            session_id: session_id.to_string(),
        }))
    }

    async fn get_live_session(
        &self,
        request: tonic::Request<LiveScriptSession>,
    ) -> Result<tonic::Response<LiveScript>, tonic::Status> {
        let request = request.get_ref();

        match self
            .live_script_manager
            .get_session(&request.script_id, &request.session_id)
            .await?
        {
            Some(v) => Ok(Response::new(LiveScript {
                session_id: Some(request.session_id.clone()),
                script_id: request.script_id.clone(),
                script_config: Some(v.script_config),
                bundle: Some(v.bundle),
            })),
            None => Err(tonic::Status::not_found("Live script session not found.")),
        }
    }

    async fn delete_live_session(
        &self,
        request: tonic::Request<LiveScriptSession>,
    ) -> Result<tonic::Response<()>, tonic::Status> {
        let request = request.get_ref();

        self.live_script_manager
            .delete_session(&request.script_id, &request.session_id)
            .await?;

        Ok(Response::new(()))
    }

    type StreamLiveLogsStream = LogMessageStream;

    async fn stream_live_logs(
        &self,
        request: tonic::Request<LiveScriptSession>,
    ) -> Result<tonic::Response<Self::StreamLiveLogsStream>, tonic::Status> {
        let request = request.get_ref();

        let stream = self
            .live_script_manager
            .log_stream(&live_log_channel(&request.session_id))
            .await?;

        Ok(Response::new(Box::pin(stream.map(Ok))))
    }

    type StreamScriptLogsStream = LogMessageStream;

    async fn stream_script_logs(
        &self,
        request: tonic::Request<StreamScriptLogsRequest>,
    ) -> Result<tonic::Response<Self::StreamScriptLogsStream>, tonic::Status> {
        let request = request.get_ref();

        let stream = self
            .live_script_manager
            .log_stream(&script_log_channel(&request.script_id))
            .await?;

        Ok(Response::new(Box::pin(stream.map(Ok))))
    }

    async fn missing_blobs(
        &self,
        request: tonic::Request<MissingBlobsRequest>,
    ) -> Result<tonic::Response<MissingBlobsResponse>, tonic::Status> {
        let missing = self.blobs.missing(&request.get_ref().hashes).await?;

        Ok(Response::new(MissingBlobsResponse { missing }))
    }

    async fn set_alias(
        &self,
        request: tonic::Request<SetAliasRequest>,
    ) -> Result<tonic::Response<Alias>, tonic::Status> {
        let request = request.get_ref();
        if let Some(reason) = alias_name_error(&request.name) {
            return Err(Status::invalid_argument(reason));
        }

        let script_id =
            Uuid::from_str(&request.script_id).map_err(|e| Status::internal(e.to_string()))?;
        let revision_id = Uuid::from_str(&request.revision_id)
            .map_err(|_| Status::invalid_argument("Revision id is not a uuid."))?;

        // An alias may only point inside its own script; checked here so the
        // upsert can never install a pointer the router would refuse.
        let owner: Option<Uuid> =
            sqlx::query_scalar("SELECT script_id FROM revisions WHERE id = $1")
                .bind(revision_id)
                .fetch_optional(&self.database)
                .await
                .map_err(|e| Status::internal(e.to_string()))?;
        if owner != Some(script_id) {
            return Err(Status::failed_precondition(
                "Revision does not exist for this script.",
            ));
        }

        sqlx::query(
            "INSERT INTO aliases (script_id, name, revision_id) VALUES ($1, $2, $3)
             ON CONFLICT (script_id, name)
             DO UPDATE SET revision_id = $3, last_updated = now()",
        )
        .bind(script_id)
        .bind(&request.name)
        .bind(revision_id)
        .execute(&self.database)
        .await
        .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(Alias {
            script_id: script_id.to_string(),
            name: request.name.clone(),
            revision_id: revision_id.to_string(),
        }))
    }

    async fn get_alias(
        &self,
        request: tonic::Request<GetAliasRequest>,
    ) -> Result<tonic::Response<Alias>, tonic::Status> {
        let request = request.get_ref();
        let script_id =
            Uuid::from_str(&request.script_id).map_err(|e| Status::internal(e.to_string()))?;

        let revision_id: Option<Uuid> = sqlx::query_scalar(
            "SELECT revision_id FROM aliases WHERE script_id = $1 AND name = $2",
        )
        .bind(script_id)
        .bind(&request.name)
        .fetch_optional(&self.database)
        .await
        .map_err(|e| Status::internal(e.to_string()))?;

        match revision_id {
            Some(revision_id) => Ok(Response::new(Alias {
                script_id: script_id.to_string(),
                name: request.name.clone(),
                revision_id: revision_id.to_string(),
            })),
            None => Err(Status::not_found("No alias with that name.")),
        }
    }

    async fn list_aliases(
        &self,
        request: tonic::Request<ListAliasesRequest>,
    ) -> Result<tonic::Response<ListAliasesResponse>, tonic::Status> {
        let script_id = Uuid::from_str(&request.get_ref().script_id)
            .map_err(|e| Status::internal(e.to_string()))?;

        let rows: Vec<(String, Uuid)> = sqlx::query_as(
            "SELECT name, revision_id FROM aliases WHERE script_id = $1 ORDER BY name",
        )
        .bind(script_id)
        .fetch_all(&self.database)
        .await
        .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(ListAliasesResponse {
            aliases: rows
                .into_iter()
                .map(|(name, revision_id)| Alias {
                    script_id: script_id.to_string(),
                    name,
                    revision_id: revision_id.to_string(),
                })
                .collect(),
        }))
    }
}

/// Why an alias name cannot be addressed by the router, if it cannot.
///
/// The subdomain form is `<ident>--<alias>`, so a name must never contain
/// the `--` marker, and the `live-`/`r-` prefixes belong to sessions and
/// revision previews.
fn alias_name_error(name: &str) -> Option<&'static str> {
    let shaped = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--");

    if !shaped {
        return Some("An alias is 1-64 lowercase letters, digits or single dashes.");
    }

    if name.starts_with("live-") || name.starts_with("r-") {
        return Some("Alias names starting with 'live-' or 'r-' are reserved for routing.");
    }

    None
}

/// The boxed stream both log rpcs return.
type LogMessageStream =
    std::pin::Pin<Box<dyn futures::Stream<Item = Result<LogMessage, Status>> + Send>>;

/// Container-backed tests driving the service against real stores.
///
/// These live here rather than in `tests/` because this crate is a binary and
/// has no library target for an integration test to import.
#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::postgres::PgPoolOptions;
    use testcontainers_modules::minio::MinIO;
    use testcontainers_modules::postgres::Postgres as PostgresImage;
    use testcontainers_modules::redis::Redis;
    use testcontainers_modules::testcontainers::runners::AsyncRunner;
    use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt};

    // The generated trait shares its name with the struct implementing it, and
    // calling delete_project needs the trait in scope.
    #[allow(unused_imports)]
    use crate::proto_script_service::script_service_server::ScriptService as ScriptServiceRpc;

    /// A service wired to a migrated postgres and a redis.
    ///
    /// The containers ride along because dropping them stops the stores.
    struct TestService {
        _postgres: ContainerAsync<PostgresImage>,
        _redis: ContainerAsync<Redis>,
        _minio: ContainerAsync<MinIO>,
        database: Pool<Postgres>,
        redis_url: String,
        service: ScriptService,
    }

    async fn service() -> TestService {
        let postgres = PostgresImage::default()
            .with_tag("17-alpine")
            .start()
            .await
            .expect("postgres starts");
        let postgres_port = postgres
            .get_host_port_ipv4(5432)
            .await
            .expect("postgres port is published");

        let redis = Redis::default().start().await.expect("redis starts");
        let redis_port = redis
            .get_host_port_ipv4(6379)
            .await
            .expect("redis port is published");

        let database = PgPoolOptions::new()
            .connect(&format!(
                "postgresql://postgres:postgres@127.0.0.1:{postgres_port}/postgres"
            ))
            .await
            .expect("postgres accepts connections");

        sqlx::migrate!("./migrations")
            .run(&database)
            .await
            .expect("migrations apply");

        let minio = MinIO::default().start().await.expect("minio starts");
        let minio_port = minio
            .get_host_port_ipv4(9000)
            .await
            .expect("minio port is published");
        let minio_endpoint = format!("http://127.0.0.1:{minio_port}");

        let blobs = crate::blob_store::BlobStore::new(crate::blob_store::BlobStoreConfig {
            endpoint: minio_endpoint,
            access_key: "minioadmin".to_owned(),
            secret_key: "minioadmin".to_owned(),
            bucket: "test-blobs".to_owned(),
        })
        .await;

        let redis_url = format!("redis://127.0.0.1:{redis_port}");
        let service =
            ScriptService::new(database.clone(), LiveScriptManager::new(&redis_url), blobs);

        TestService {
            _postgres: postgres,
            _redis: redis,
            _minio: minio,
            database,
            redis_url,
            service,
        }
    }

    async fn insert_script(database: &Pool<Postgres>, identifier: &str, project_id: Uuid) -> Uuid {
        let (id,): (Uuid,) = sqlx::query_as(
            "INSERT INTO scripts (public_identifier, project_id) VALUES ($1, $2) RETURNING id",
        )
        .bind(identifier)
        .bind(project_id)
        .fetch_one(database)
        .await
        .expect("script inserts");

        id
    }

    #[tokio::test]
    async fn deleting_a_project_removes_its_scripts_and_their_live_sessions() {
        let harness = service().await;
        let project_id = Uuid::new_v4();

        let script_id = insert_script(&harness.database, "doomed", project_id).await;

        let session_id = harness
            .service
            .live_script_manager
            .put_session(LiveScript {
                session_id: None,
                script_id: script_id.to_string(),
                script_config: Some(crate::proto_script_service::ScriptConfig {
                    id: script_id.to_string(),
                    entry_point: "main.lua".to_owned(),
                    includes: vec![],
                    ignore: vec![],
                    capabilities: None,
                }),
                bundle: Some(Bundle {
                    entry_point: "main.lua".to_owned(),
                    files: vec![],
                }),
            })
            .await
            .expect("session is created");

        harness
            .service
            .delete_project(tonic::Request::new(DeleteProjectRequest {
                project_id: project_id.to_string(),
            }))
            .await
            .expect("project deletes");

        let remaining: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM scripts WHERE project_id = $1")
                .bind(project_id)
                .fetch_one(&harness.database)
                .await
                .expect("count runs");
        assert_eq!(remaining.0, 0, "scripts outlived their project");

        // Finding the scripts to clean up is a separate query from deleting
        // them, and it naming a column that does not exist is invisible unless
        // the sessions are checked.
        assert!(
            harness
                .service
                .live_script_manager
                .get_session(&script_id.to_string(), &session_id.to_string())
                .await
                .expect("session lookup runs")
                .is_none(),
            "live session outlived the project"
        );
    }

    #[tokio::test]
    async fn deleting_a_project_leaves_other_projects_alone() {
        let harness = service().await;

        let doomed = Uuid::new_v4();
        let survivor = Uuid::new_v4();
        insert_script(&harness.database, "doomed", doomed).await;
        insert_script(&harness.database, "survivor", survivor).await;

        harness
            .service
            .delete_project(tonic::Request::new(DeleteProjectRequest {
                project_id: doomed.to_string(),
            }))
            .await
            .expect("project deletes");

        let remaining: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM scripts WHERE project_id = $1")
                .bind(survivor)
                .fetch_one(&harness.database)
                .await
                .expect("count runs");
        assert_eq!(remaining.0, 1);
    }

    #[tokio::test]
    async fn a_published_log_line_reaches_the_live_log_stream() {
        use deadpool_redis::redis::AsyncCommands;

        let harness = service().await;

        let mut stream = harness
            .service
            .stream_live_logs(tonic::Request::new(LiveScriptSession {
                script_id: "unused-by-the-channel".to_owned(),
                session_id: "session-log-test".to_owned(),
            }))
            .await
            .expect("stream opens")
            .into_inner();

        // Publishing happens after subscribing because pub/sub has no replay.
        // The garbage line first proves a malformed publisher cannot end the
        // stream before the real line arrives.
        let client = deadpool_redis::redis::Client::open(harness.redis_url.as_str())
            .expect("redis client builds");
        let mut connection = client
            .get_async_connection()
            .await
            .expect("redis accepts connections");

        let channel = live_log_channel("session-log-test");
        let line = actias_common::logging::LogLine {
            level: "info".to_owned(),
            message: "hello logs".to_owned(),
            timestamp_ms: 123,
        };

        let _: () = connection
            .publish(&channel, "this is not json")
            .await
            .expect("garbage publishes");
        let _: () = connection
            .publish(&channel, serde_json::to_string(&line).expect("serializes"))
            .await
            .expect("line publishes");

        let received = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await
            .expect("a line arrives in time")
            .expect("the stream stays open")
            .expect("the line is ok");

        assert_eq!(received.level, "info");
        assert_eq!(received.message, "hello logs");
        assert_eq!(received.timestamp_ms, 123);
    }

    #[tokio::test]
    async fn bundles_round_trip_through_the_blob_store() {
        let harness = service().await;
        let project = Uuid::new_v4();
        let script_id = insert_script(&harness.database, "blobbed", project).await;

        let source = b"on \"fetch\" (function() end)".to_vec();
        let make_request = |content: Vec<u8>, hash: String| CreateRevisionRequest {
            script_id: script_id.to_string(),
            script_config: Some(crate::proto_script_service::ScriptConfig {
                id: script_id.to_string(),
                entry_point: "main.lua".to_owned(),
                includes: vec![],
                ignore: vec![],
                capabilities: None,
            }),
            bundle: Some(Bundle {
                entry_point: "main.lua".to_owned(),
                files: vec![File {
                    file_path: "main.lua".to_owned(),
                    content,
                    hash,
                    ..Default::default()
                }],
            }),
        };

        // First publish carries content inline; the store hashes and keeps it.
        let created = harness
            .service
            .create_revision(tonic::Request::new(make_request(
                source.clone(),
                String::new(),
            )))
            .await
            .expect("revision creates")
            .into_inner();

        let stored_hash = blake3::hash(&source).to_hex().to_string();

        // The negotiation must now consider the hash present, and an unknown
        // hash missing.
        let missing = harness
            .service
            .missing_blobs(tonic::Request::new(MissingBlobsRequest {
                hashes: vec![stored_hash.clone(), "0".repeat(64)],
            }))
            .await
            .expect("negotiation answers")
            .into_inner()
            .missing;
        assert_eq!(missing, vec!["0".repeat(64)]);

        // A manifest-only publish referencing the stored hash succeeds
        // without any content, which is what makes republish near-free.
        harness
            .service
            .create_revision(tonic::Request::new(make_request(
                vec![],
                stored_hash.clone(),
            )))
            .await
            .expect("manifest-only revision creates");

        // A manifest-only publish of an unstored hash is refused.
        let refused = harness
            .service
            .create_revision(tonic::Request::new(make_request(vec![], "1".repeat(64))))
            .await;
        assert!(refused.is_err(), "an unstored hash must be refused");

        // Reading back hydrates the content from the blob store.
        let revision = harness
            .service
            .get_revision(tonic::Request::new(GetRevisionRequest {
                id: created.id.clone(),
                with_bundle: true,
                manifest_only: false,
            }))
            .await
            .expect("revision reads")
            .into_inner();

        let files = revision.bundle.expect("bundle present").files;
        assert_eq!(files[0].content, source);
        assert_eq!(files[0].hash, stored_hash);

        // A manifest-only read carries everything but the bytes; callers
        // with blob store access pull those themselves.
        let manifest = harness
            .service
            .get_revision(tonic::Request::new(GetRevisionRequest {
                id: created.id,
                with_bundle: true,
                manifest_only: true,
            }))
            .await
            .expect("manifest reads")
            .into_inner();

        let files = manifest.bundle.expect("bundle present").files;
        assert!(files[0].content.is_empty(), "manifest must not carry bytes");
        assert_eq!(files[0].hash, stored_hash);
        assert_eq!(files[0].size, source.len() as u64);
    }

    #[tokio::test]
    async fn the_stored_contract_is_derived_from_the_code_not_the_claim() {
        let harness = service().await;
        let project = Uuid::new_v4();
        let script_id = insert_script(&harness.database, "derived", project).await;

        let request = |content: &[u8]| CreateRevisionRequest {
            script_id: script_id.to_string(),
            script_config: Some(crate::proto_script_service::ScriptConfig {
                id: script_id.to_string(),
                entry_point: "main.lua".to_owned(),
                includes: vec![],
                ignore: vec![],
                // The claim lies; the stored contract must not.
                capabilities: Some(crate::proto_script_service::Capabilities {
                    kv: vec!["a-lie".to_owned()],
                    events: vec![],
                    secrets: vec!["also-a-lie".to_owned()],
                    objects: vec!["a-lied-class".to_owned()],
                }),
            }),
            bundle: Some(Bundle {
                entry_point: "main.lua".to_owned(),
                files: vec![File {
                    file_path: "main.lua".to_owned(),
                    content: content.to_vec(),
                    ..Default::default()
                }],
            }),
        };

        let created = harness
            .service
            .create_revision(tonic::Request::new(request(
                br#"local t = kv "truth" on "fetch" (function() end)"#,
            )))
            .await
            .expect("revision creates")
            .into_inner();

        let contract = created
            .script_config
            .expect("config present")
            .capabilities
            .expect("contract present");
        assert_eq!(contract.kv, vec!["truth"]);
        assert_eq!(contract.events, vec!["fetch"]);
        assert!(contract.secrets.is_empty(), "the claimed secret survived");

        // Code that does not parse cannot publish at all.
        let refused = harness
            .service
            .create_revision(tonic::Request::new(request(b"this is ((( not lua")))
            .await;
        assert!(refused.is_err(), "unparseable code must be refused");
    }

    #[tokio::test]
    async fn an_alias_is_a_movable_pointer_within_one_script() {
        let harness = service().await;
        let project = Uuid::new_v4();
        let script_id = insert_script(&harness.database, "aliased", project).await;
        let other_script = insert_script(&harness.database, "other", project).await;

        let publish = |script: Uuid, body: &'static [u8]| {
            let service = &harness.service;
            async move {
                service
                    .create_revision(tonic::Request::new(CreateRevisionRequest {
                        script_id: script.to_string(),
                        script_config: Some(crate::proto_script_service::ScriptConfig {
                            id: script.to_string(),
                            entry_point: "main.lua".to_owned(),
                            includes: vec![],
                            ignore: vec![],
                            capabilities: None,
                        }),
                        bundle: Some(Bundle {
                            entry_point: "main.lua".to_owned(),
                            files: vec![File {
                                file_path: "main.lua".to_owned(),
                                content: body.to_vec(),
                                ..Default::default()
                            }],
                        }),
                    }))
                    .await
                    .expect("revision creates")
                    .into_inner()
            }
        };

        let first = publish(script_id, b"on \"fetch\" (function() return 1 end)").await;
        let second = publish(script_id, b"on \"fetch\" (function() return 2 end)").await;
        let foreign = publish(other_script, b"on \"fetch\" (function() end)").await;

        let set = |name: &str, revision: String| {
            let service = &harness.service;
            let script = script_id.to_string();
            let name = name.to_owned();
            async move {
                service
                    .set_alias(tonic::Request::new(SetAliasRequest {
                        script_id: script,
                        name,
                        revision_id: revision,
                    }))
                    .await
            }
        };

        // Create, then move: the same upsert call.
        set("staging", first.id.clone()).await.expect("alias sets");
        set("staging", second.id.clone())
            .await
            .expect("alias moves");

        let resolved = harness
            .service
            .get_alias(tonic::Request::new(GetAliasRequest {
                script_id: script_id.to_string(),
                name: "staging".to_owned(),
            }))
            .await
            .expect("alias resolves")
            .into_inner();
        assert_eq!(resolved.revision_id, second.id);

        let listed = harness
            .service
            .list_aliases(tonic::Request::new(ListAliasesRequest {
                script_id: script_id.to_string(),
            }))
            .await
            .expect("aliases list")
            .into_inner()
            .aliases;
        assert_eq!(listed.len(), 1, "the move must not create a second row");

        // A pointer outside the script, and names the router cannot
        // address, are refused.
        let refused = set("staging", foreign.id).await;
        assert!(refused.is_err(), "a foreign revision must be refused");
        for name in ["live-x", "r-1", "has--marker", "-lead", "Upper"] {
            let refused = set(name, second.id.clone()).await;
            assert!(refused.is_err(), "name {name:?} must be refused");
        }
    }
}
