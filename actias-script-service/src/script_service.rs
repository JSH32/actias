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

/// The contract arrays an object owner can be resolved from; a closed
/// set, so the JSONB member queried below is selected by match, never
/// taken from input.
#[derive(Clone, Copy)]
enum ContractMember {
    /// `on "queue:<name>"`, the queue's consumer.
    Events,
    /// `workflow "name"`, the definition's declarer.
    Workflows,
    /// `queue "name"`, a producer.
    Queues,
    /// `database "name"`, a declarer.
    Databases,
    /// `object "Class" { ... }`, the class's declarer.
    Objects,
}

impl ContractMember {
    /// Resolution order per class: a queue's consumer outranks its
    /// producers; databases and user classes read one member each.
    /// [`None`] for platform classes never resolved this way (`__cron`
    /// scopes to its script and never asks).
    fn for_class(class: &str) -> Option<&'static [ContractMember]> {
        match class {
            actias_common::classes::QUEUE_CLASS => {
                Some(&[ContractMember::Events, ContractMember::Queues])
            }
            actias_common::classes::DATABASE_CLASS => Some(&[ContractMember::Databases]),
            actias_common::classes::WORKFLOW_CLASS => Some(&[ContractMember::Workflows]),
            class if class.starts_with("__") => None,
            _ => Some(&[ContractMember::Objects]),
        }
    }

    /// The JSONB member under `capabilities` holding the declarations.
    fn member(&self) -> &'static str {
        match self {
            ContractMember::Events => "events",
            ContractMember::Workflows => "workflows",
            ContractMember::Queues => "queues",
            ContractMember::Databases => "databases",
            ContractMember::Objects => "objects",
        }
    }

    /// What the declaration reads as inside that member.
    fn needle(&self, class: &str, name: &str) -> String {
        match self {
            ContractMember::Events => format!("queue:{name}"),
            // A workflow instance is `<definition>/<caller id>`; the
            // definition segment is what the contract declares.
            ContractMember::Workflows => name.split('/').next().unwrap_or_default().to_owned(),
            ContractMember::Queues | ContractMember::Databases => name.to_owned(),
            ContractMember::Objects => class.to_owned(),
        }
    }
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

    /// The script in the project whose current contract declares the
    /// needle in the given member. Ordering by id makes a multi-declarer
    /// pick stable rather than meaningful; publish-time uniqueness keeps
    /// queues and user classes single-owner, databases are shared by
    /// design.
    async fn contract_owner(
        &self,
        project_id: Uuid,
        member: ContractMember,
        class: &str,
        name: &str,
    ) -> Result<Option<Uuid>, tonic::Status> {
        // A declaration may carry an annotation after '=' (a database's
        // migrations directory, a publish policy); ownership reads the
        // name alone, so the comparison strips the annotation.
        let sql = format!(
            "SELECT s.id FROM scripts s
             JOIN revisions r ON r.id = s.current_revision,
             LATERAL jsonb_array_elements_text(
                 r.script_config->'capabilities'->'{}'
             ) AS held(name)
             WHERE s.project_id = $1
               AND split_part(held.name, '=', 1) = $2
             ORDER BY s.id LIMIT 1",
            member.member()
        );

        sqlx::query_scalar(&sql)
            .bind(project_id)
            .bind(member.needle(class, name))
            .fetch_optional(&self.database)
            .await
            .map_err(|e| Status::internal(e.to_string()))
    }

    /// Refuses a derived contract colliding with a sibling script's
    /// current one: a queue has one consumer (`on "queue:<name>"`) and a
    /// user class one declarer per project. Producers (`queue "name"`)
    /// and databases repeat freely; the publishing script's own previous
    /// revision never conflicts with itself.
    async fn refuse_contract_conflicts(
        &self,
        script_id: &Uuid,
        capabilities: &crate::database_types::Capabilities,
    ) -> Result<(), tonic::Status> {
        let consumed: Vec<String> = capabilities
            .events
            .iter()
            .filter(|event| event.starts_with("queue:"))
            .cloned()
            .collect();

        // The member names are the closed set ContractMember also reads;
        // deliberate literals, never taken from input.
        let checks = [
            ("events", consumed, "already consumes"),
            ("objects", capabilities.objects.clone(), "already declares"),
            // A workflow definition has one declarer: its runs replay
            // that script's revisions, so a second declarer would split
            // the identity space.
            (
                "workflows",
                capabilities.workflows.clone(),
                "already declares",
            ),
        ];

        for (member, names, verb) in checks {
            if names.is_empty() {
                continue;
            }

            let sql = format!(
                "SELECT s.public_identifier, held.name
                 FROM scripts s
                 JOIN revisions r ON r.id = s.current_revision,
                 LATERAL jsonb_array_elements_text(
                     r.script_config->'capabilities'->'{member}'
                 ) AS held(name)
                 WHERE s.project_id = (SELECT project_id FROM scripts WHERE id = $1)
                   AND s.id <> $1
                   AND held.name = ANY($2)
                 LIMIT 1"
            );

            let conflict: Option<(String, String)> = sqlx::query_as(&sql)
                .bind(script_id)
                .bind(&names)
                .fetch_optional(&self.database)
                .await
                .map_err(|e| Status::internal(e.to_string()))?;

            if let Some((identifier, name)) = conflict {
                return Err(Status::failed_precondition(format!(
                    "Script '{identifier}' {verb} '{name}' in this project; \
                     it has exactly one owner. Remove one declaration and publish again."
                )));
            }
        }

        Ok(())
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
            databases: derived.databases,
            queues: derived.queues,
            workflows: derived.workflows,
            workflow_steps: derived.workflow_steps,
            publishes: derived.publishes,
        });

        // Identity is project-scoped, so single-owner declarations must be
        // unique across the project: publishing the second consumer of a
        // queue or the second declarer of a class fails loudly here.
        if let Some(capabilities) = script_config.capabilities.as_ref() {
            self.refuse_contract_conflicts(script_id, capabilities)
                .await?;
        }

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
            // Already a Status; re-wrapping would flatten every refusal
            // (bad code, contract conflicts) into an internal error.
            .await?,
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

    async fn resolve_class_owner(
        &self,
        request: tonic::Request<ResolveClassOwnerRequest>,
    ) -> Result<tonic::Response<ClassOwner>, tonic::Status> {
        let request = request.get_ref();
        let project_id = Uuid::from_str(&request.project_id)
            .map_err(|_| Status::invalid_argument("'project_id' was not a valid uuid"))?;

        let reads = ContractMember::for_class(&request.class).ok_or_else(|| {
            Status::invalid_argument("Platform class has no contract-derived owner.")
        })?;

        // Contracts first, in declaration-strength order (a queue's
        // consumer outranks its producers); the directory is the fallback
        // so orphaned data (declaring revision gone) stays reachable.
        for member in reads {
            if let Some(script_id) = self
                .contract_owner(project_id, *member, &request.class, &request.name)
                .await?
            {
                return Ok(Response::new(ClassOwner {
                    script_id: script_id.to_string(),
                }));
            }
        }

        let remembered: Option<Uuid> = sqlx::query_scalar(
            "SELECT script_id FROM object_instances
             WHERE scope_id = $1 AND class = $2 AND name = $3",
        )
        .bind(project_id)
        .bind(&request.class)
        .bind(&request.name)
        .fetch_optional(&self.database)
        .await
        .map_err(|e| Status::internal(e.to_string()))?;

        match remembered {
            Some(script_id) => Ok(Response::new(ClassOwner {
                script_id: script_id.to_string(),
            })),
            None => Err(Status::not_found(
                "No current contract in the project owns that identity.",
            )),
        }
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

    /// Publishes a bare revision carrying the given capability contract
    /// and makes it the script's current one.
    async fn publish_contract(
        database: &Pool<Postgres>,
        script_id: Uuid,
        capabilities: serde_json::Value,
    ) {
        let config = serde_json::json!({
            "id": script_id,
            "entryPoint": "main.lua",
            "includes": [],
            "ignore": [],
            "capabilities": capabilities,
        });
        let (revision_id,): (Uuid,) = sqlx::query_as(
            "INSERT INTO revisions (script_id, entry_point, script_config)
             VALUES ($1, 'main.lua', $2) RETURNING id",
        )
        .bind(script_id)
        .bind(sqlx::types::Json(config))
        .fetch_one(database)
        .await
        .expect("revision inserts");

        sqlx::query("UPDATE scripts SET current_revision = $2 WHERE id = $1")
            .bind(script_id)
            .bind(revision_id)
            .execute(database)
            .await
            .expect("current revision sets");
    }

    /// A full capability contract with only the given members filled.
    fn contract(members: &[(&str, &[&str])]) -> serde_json::Value {
        let mut capabilities = serde_json::json!({
            "kv": [], "events": [], "secrets": [],
            "objects": [], "databases": [], "queues": [],
        });
        for (member, names) in members {
            capabilities[member] = serde_json::json!(names);
        }
        capabilities
    }

    /// Publishes `code` as a one-file bundle through the real rpc, the
    /// path every client takes; the contract is derived from the code.
    async fn publish_code(
        harness: &TestService,
        script_id: Uuid,
        code: &str,
    ) -> Result<tonic::Response<Revision>, tonic::Status> {
        harness
            .service
            .create_revision(tonic::Request::new(CreateRevisionRequest {
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
                        content: code.as_bytes().to_vec(),
                        hash: String::new(),
                        size: code.len() as u64,
                        content_type: "text/x-lua".to_owned(),
                        kind: crate::bundle::FileKind::Module as i32,
                    }],
                }),
            }))
            .await
    }

    #[tokio::test]
    async fn a_queue_has_one_consumer_and_a_class_one_declarer_per_project() {
        let harness = service().await;
        let project = Uuid::new_v4();

        let first = insert_script(&harness.database, "first", project).await;
        let second = insert_script(&harness.database, "second", project).await;

        publish_code(
            &harness,
            first,
            "on \"queue:jobs\" (function(msg) end)\nlocal Room = object \"Room\" { }",
        )
        .await
        .expect("the first consumer and declarer publish");

        // A second consumer of the same queue fails its publish loudly.
        let refused = publish_code(&harness, second, "on \"queue:jobs\" (function(msg) end)")
            .await
            .expect_err("a second consumer must be refused");
        assert_eq!(refused.code(), tonic::Code::FailedPrecondition);
        assert!(
            refused.message().contains("first"),
            "the refusal names the incumbent: {refused}"
        );

        // A second declarer of the same class fails the same way.
        let refused = publish_code(&harness, second, "local Room = object \"Room\" { }")
            .await
            .expect_err("a second declarer must be refused");
        assert_eq!(refused.code(), tonic::Code::FailedPrecondition);

        // Producing to the queue is free: many scripts may send.
        publish_code(&harness, second, "local jobs = queue \"jobs\"")
            .await
            .expect("a producer publishes");

        // The incumbent republishing its own declarations stays fine.
        publish_code(
            &harness,
            first,
            "on \"queue:jobs\" (function(msg) end)\nlocal Room = object \"Room\" { }",
        )
        .await
        .expect("the owner republishes itself");
    }

    #[tokio::test]
    async fn class_owners_resolve_from_contracts_then_the_directory() {
        let harness = service().await;
        let project = Uuid::new_v4();

        let producer = insert_script(&harness.database, "producer", project).await;
        let consumer = insert_script(&harness.database, "consumer", project).await;
        publish_contract(
            &harness.database,
            producer,
            contract(&[("queues", &["jobs"])]),
        )
        .await;
        publish_contract(
            &harness.database,
            consumer,
            contract(&[
                ("events", &["queue:jobs"]),
                ("objects", &["Room"]),
                // The annotation after '=' names the migrations
                // directory; ownership still resolves by name alone.
                ("databases", &["main=migrations/main"]),
            ]),
        )
        .await;

        let resolve = |class: &str, name: &str| {
            let request = ResolveClassOwnerRequest {
                project_id: project.to_string(),
                class: class.to_owned(),
                name: name.to_owned(),
            };
            harness
                .service
                .resolve_class_owner(tonic::Request::new(request))
        };

        // The consumer outranks the producer for a queue's code.
        let owner = resolve("__queue", "jobs").await.expect("resolves");
        assert_eq!(owner.get_ref().script_id, consumer.to_string());

        // A user class resolves its declarer.
        let owner = resolve("Room", "lobby").await.expect("resolves");
        assert_eq!(owner.get_ref().script_id, consumer.to_string());

        // An annotated declaration resolves by its bare name.
        let owner = resolve("__database", "main").await.expect("resolves");
        assert_eq!(owner.get_ref().script_id, consumer.to_string());

        // With no consumer anywhere, a producer's declaration suffices.
        let solo = Uuid::new_v4();
        let lone_producer = insert_script(&harness.database, "lone", solo).await;
        publish_contract(
            &harness.database,
            lone_producer,
            contract(&[("queues", &["only-sent"])]),
        )
        .await;
        let request = ResolveClassOwnerRequest {
            project_id: solo.to_string(),
            class: "__queue".to_owned(),
            name: "only-sent".to_owned(),
        };
        let owner = harness
            .service
            .resolve_class_owner(tonic::Request::new(request))
            .await
            .expect("resolves");
        assert_eq!(owner.get_ref().script_id, lone_producer.to_string());

        // Orphaned data: no current contract declares it, but the
        // directory remembers whose code claimed it.
        let ghost_owner = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO object_instances (scope_id, class, name, script_id)
             VALUES ($1, '__queue', 'ghost', $2)",
        )
        .bind(project)
        .bind(ghost_owner)
        .execute(&harness.database)
        .await
        .expect("directory row inserts");
        let owner = resolve("__queue", "ghost").await.expect("resolves");
        assert_eq!(owner.get_ref().script_id, ghost_owner.to_string());

        // Nothing anywhere: NOT_FOUND, so the worker can say so cleanly.
        let missing = resolve("__queue", "never-was").await;
        assert_eq!(
            missing.expect_err("must not resolve").code(),
            tonic::Code::NotFound
        );
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
                    databases: vec![],
                    queues: vec![],
                    workflows: vec![],
                    workflow_steps: vec![],
                    publishes: vec![],
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
