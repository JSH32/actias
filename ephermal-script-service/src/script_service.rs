use lz4_flex::{compress_prepend_size, decompress_size_prepended};
use mongodb::options::FindOneOptions;
use mongodb::{bson::doc, options::IndexOptions, Collection, Database, IndexModel};
use tonic::{Response, Status};

use crate::proto_script_service::{self, script_service_server, Script};

use crate::proto_script_service::find_script_request::Query::{Id, PublicName};

pub struct ScriptService {
    scripts: Collection<Script>,
}

impl ScriptService {
    pub async fn new(database: &Database) -> Self {
        let collection = database.collection::<Script>("scripts");
        collection
            .create_index(
                IndexModel::builder()
                    .keys(doc! {"public_identifier": 1})
                    .options(IndexOptions::builder().unique(true).build())
                    .build(),
                None,
            )
            .await
            .unwrap();

        Self {
            scripts: collection,
        }
    }
}

#[tonic::async_trait]
impl script_service_server::ScriptService for ScriptService {
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

        let inserted_id = self
            .scripts
            .insert_one(
                Script {
                    id: None,
                    public_identifier: request.public_identifier,
                    bundle: Some(bundle),
                },
                None,
            )
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .inserted_id;

        Ok(Response::new(
            self.scripts
                .find_one(doc! {"_id": inserted_id}, None)
                .await
                .map_err(|e| Status::internal(e.to_string()))?
                .unwrap(),
        ))
    }

    async fn query_script(
        &self,
        request: tonic::Request<proto_script_service::FindScriptRequest>,
    ) -> Result<tonic::Response<proto_script_service::Script>, tonic::Status> {
        let request = request.get_ref().clone();

        let mut response = self
            .scripts
            .find_one(
                match request.query.unwrap() {
                    Id(id) => doc! {"_id": id},
                    PublicName(name) => doc! {"public_identifier": name},
                },
                FindOneOptions::builder()
                    .projection(doc! {
                        "_id": 1,
                        "public_identifier": 1,
                        "bundle": request.include_bundle
                    })
                    .build(),
            )
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or(Status::not_found("Script not found"))?;

        let mut bundle = response.bundle.unwrap();

        // Decompress from DB.
        for file in bundle.files.iter_mut() {
            file.content = decompress_size_prepended(&file.content).unwrap();
        }

        response.bundle = Some(bundle);

        Ok(Response::new(response))
    }
}
