use lz4_flex::{compress_prepend_size, decompress_size_prepended};
use sqlx::postgres::PgRow;
use sqlx::types::chrono::Utc;
use sqlx::types::{chrono, Json, Uuid};
use sqlx::{Pool, Postgres, Row};
use tonic::{Response, Status};

use crate::bundle::Bundle;
use crate::proto_script_service::find_script_request::{self, RevisionRequestType};
use crate::proto_script_service::{self, script_service_server, Revision, Script};

use crate::proto_script_service::find_script_request::Query::{Id, PublicName};

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
    ) -> Result<Script, sqlx::Error> {
        sqlx::query(&format!(
            "SELECT * FROM scripts WHERE {} = $1",
            match &script_query {
                Id(_) => "id",
                PublicName(_) => "public_identifier",
            }
        ))
        .bind(match script_query {
            Id(v) => v,
            PublicName(v) => v,
        })
        .map(map_script)
        .fetch_one(&self.database)
        .await
    }

    async fn create_db_revision(
        &self,
        script_id: &str,
        bundle: Bundle,
    ) -> Result<Revision, sqlx::Error> {
        // Create revision
        sqlx::query("INSERT INTO revisions (script_id, bundle) VALUES ($1, $2) RETURNING *")
            .bind(script_id)
            .bind(serde_json::to_value(bundle).unwrap())
            .map(map_revision)
            .fetch_one(&self.database)
            .await
    }
}

#[tonic::async_trait]
impl script_service_server::ScriptService for ScriptService {
    async fn create_revision(
        &self,
        request: tonic::Request<proto_script_service::CreateRevisionRequest>,
    ) -> Result<tonic::Response<proto_script_service::Revision>, tonic::Status> {
        let request = request.get_ref().clone();

        let script_info = match self
            .get_script_info(find_script_request::Query::Id(request.script_id.clone()))
            .await
        {
            Ok(v) => v,
            Err(e) => {
                return Err(match e {
                    sqlx::Error::RowNotFound => {
                        Status::not_found("Script with that id was not found")
                    }
                    _ => Status::internal(e.to_string()),
                })
            }
        };

        Ok(Response::new(
            self.create_db_revision(&script_info.id, request.bundle)
                .await
                .map_err(|e| Status::internal(e.to_string()))?,
        ))
    }

    async fn create_script(
        &self,
        request: tonic::Request<proto_script_service::CreateScriptRequest>,
    ) -> Result<tonic::Response<proto_script_service::Script>, tonic::Status> {
        let request = request.get_ref().clone();

        let mut bundle = request.bundle.clone();

        // Compress when storing in DB
        for file in bundle.files.iter_mut() {
            file.content = compress_prepend_size(&file.content);
        }

        // Check if exists, if not, then create.
        let mut script_info = match self
            .get_script_info(find_script_request::Query::PublicName(
                request.public_identifier.clone(),
            ))
            .await
        {
            Ok(v) => v,
            Err(e) => match e {
                sqlx::Error::RowNotFound => {
                    // Create a script.
                    sqlx::query("INSERT INTO scripts (public_identifier) VALUES ($1) RETURNING *")
                        .bind(request.public_identifier)
                        .map(map_script)
                        .fetch_one(&self.database)
                        .await
                        .map_err(|e| Status::internal(e.to_string()))?
                }
                _ => return Err(Status::internal(e.to_string())),
            },
        };

        // Create revision
        script_info.revisions = vec![self
            .create_db_revision(&script_info.id, request.bundle)
            .await
            .map_err(|e| Status::internal(e.to_string()))?];

        Ok(Response::new(script_info))
    }

    async fn query_script(
        &self,
        request: tonic::Request<proto_script_service::FindScriptRequest>,
    ) -> Result<tonic::Response<proto_script_service::Script>, tonic::Status> {
        let request = request.get_ref().clone();

        let mut script_info = self
            .get_script_info(request.query.unwrap())
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        script_info.revisions =
            match RevisionRequestType::from_i32(request.revision_request_type).unwrap() {
                RevisionRequestType::All => sqlx::query(
                    "SELECT * FROM revisions WHERE script_id = $1 ORDER BY created DESC",
                )
                .bind(script_info.id.clone())
                .map(map_revision)
                .fetch_all(&self.database)
                .await
                .map_err(|e| Status::internal(e.to_string()))?,
                // Only get latest verision.
                RevisionRequestType::Latest => vec![sqlx::query(
                    "SELECT * FROM revisions WHERE script_id = $1 ORDER BY created DESC LIMIT 1",
                )
                .bind(script_info.id.clone())
                .map(map_revision)
                .fetch_one(&self.database)
                .await
                .map_err(|e| Status::internal(e.to_string()))?],
                _ => vec![],
            };

        Ok(Response::new(script_info))
    }
}

fn map_script(pgrow: PgRow) -> Script {
    Script {
        id: pgrow.get::<Uuid, _>("id").to_string(),
        public_identifier: pgrow.get("public_identifier"),
        last_updated: pgrow
            .get::<chrono::DateTime<Utc>, _>("last_updated")
            .to_string(),
        revisions: Vec::new(),
    }
}

fn map_revision(pgrow: PgRow) -> Revision {
    let mut bundle: Json<Bundle> = pgrow.get("bundle");
    // Decompress from DB.
    for file in bundle.files.iter_mut() {
        file.content = decompress_size_prepended(&file.content).unwrap();
    }

    Revision {
        id: pgrow.get::<Uuid, _>("id").to_string(),
        created: pgrow.get::<chrono::DateTime<Utc>, _>("created").to_string(),
        script_id: pgrow.get::<Uuid, _>("script_id").to_string(),
        bundle: bundle.0,
    }
}
