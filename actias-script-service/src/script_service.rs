use std::str::FromStr;

use lz4_flex::{compress_prepend_size, decompress_size_prepended};
use sqlx::types::Uuid;
use sqlx::{Pool, Postgres};
use tonic::{Response, Status};

use crate::bundle::{Bundle, File};
use crate::database_types::{DbFile, DbRevision, DbScript, ScriptConfig};
use crate::proto_script_service::find_script_request::{self};
use crate::proto_script_service::{
    script_service_server, ListRevisionResponse, ListScriptResponse, Revision, Script, *,
};

use crate::proto_script_service::find_script_request::Query::{Id, PublicName};
use crate::util::safe_divide;

pub struct ScriptService {
    database: Pool<Postgres>,
}

impl ScriptService {
    pub fn new(database: Pool<Postgres>) -> Self {
        Self { database }
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
                Uuid::parse_str(&v)
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
                Uuid::parse_str(&revision_id)
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
        script_config: ScriptConfig,
        mut bundle: Bundle,
    ) -> Result<Revision, tonic::Status> {
        if &script_config.id != script_id {
            return Err(Status::invalid_argument(
                "Script config contains a different ID than the target.",
            ));
        }

        for file in bundle.files.iter_mut() {
            file.content = compress_prepend_size(&file.clone().content);
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
            sqlx::query("INSERT INTO files (revision_id, content, file_name, file_path) VALUES ($1, $2, $3, $4)")
                .bind(revision_info.id)
                .bind(&file.content)
                .bind(&file.file_name)
                .bind(&file.file_path)
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
            script_config: serde_json::to_string(&revision_info.script_config).unwrap(),
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

        sqlx::query("DELETE FROM scripts WHERE project_id = $1")
            .bind(&project_id)
            .execute(&self.database)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

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
                serde_json::from_str(&request.script_config)
                    .map_err(|_| Status::invalid_argument("Project config is not valid JSON."))?,
                request.bundle,
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
                SELECT f.file_name, f.file_path, f.content, f.revision_id
                FROM files f, revisions r 
                WHERE revision_id = $1 AND r.id = f.revision_id
                "#,
            )
            .bind(&revision_info.id)
            .fetch_all(&self.database)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

            bundle = Some(Bundle {
                entry_point: revision_info.entry_point,
                files: files
                    .iter()
                    .map(|f| File {
                        content: decompress_size_prepended(&f.content).unwrap(),
                        file_name: f.file_name.clone(),
                        file_path: f.file_path.clone(),
                        revision_id: f.revision_id.to_string(),
                    })
                    .collect(),
            })
        };

        Ok(Response::new(Revision {
            id: revision_info.id.to_string(),
            created: revision_info.created.to_string(),
            script_id: revision_info.script_id.to_string(),
            script_config: serde_json::to_string(&revision_info.script_config).unwrap(),
            bundle,
        }))
    }

    async fn list_revisions(
        &self,
        request: tonic::Request<ListRevisionsRequest>,
    ) -> Result<tonic::Response<ListRevisionResponse>, tonic::Status> {
        let request = request.get_ref();

        if request.page < 0 {
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
        .bind(request.page_size * request.page);

        if let Some(script_id) = &request.script_id {
            let uuid = Uuid::from_str(&script_id).map_err(|e| Status::internal(e.to_string()))?;

            count_query = count_query.bind(uuid.clone());
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
                    script_config: serde_json::to_string(&r.script_config).unwrap(),
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
            .bind(&revision.id)
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

        if request.page < 0 {
            return Err(Status::invalid_argument("invalid page number provided!"));
        }

        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) as count FROM scripts")
            .fetch_one(&self.database)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(ListScriptResponse {
            page: request.page,
            total_pages: safe_divide!(count.0 as i32, request.page_size),
            scripts: sqlx::query_as::<_, DbScript>(
                "SELECT * FROM scripts ORDER BY last_updated DESC LIMIT $1 OFFSET $2",
            )
            .bind(request.page_size)
            .bind(request.page_size * request.page)
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
                ))
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
}
