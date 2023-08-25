use tonic::{Response, Status};

use crate::{
    database::Database,
    proto_kv_service::{
        self, kv_service_server, DeletePairsRequest, ListNamespacesRequest, ListNamespacesResponse,
        ListPairsRequest, ListPairsResponse, PairRequest, SetPairsRequest,
    },
};

pub struct KvService {
    database: Database,
}

impl KvService {
    pub fn new(database: Database) -> Self {
        Self { database }
    }
}

#[tonic::async_trait]
impl kv_service_server::KvService for KvService {
    async fn list_namespaces(
        &self,
        request: tonic::Request<ListNamespacesRequest>,
    ) -> Result<tonic::Response<ListNamespacesResponse>, tonic::Status> {
        let request = request.get_ref();

        Ok(Response::new(
            self.database
                .get_namespaces(&request.project_id)
                .await
                .map_err(|e| Status::internal(e.to_string()))?,
        ))
    }

    async fn list_pairs(
        &self,
        request: tonic::Request<ListPairsRequest>,
    ) -> Result<tonic::Response<ListPairsResponse>, tonic::Status> {
        let request = request.get_ref();

        Ok(Response::new(
            self.database
                .list(
                    &request.project_id,
                    &request.namespace,
                    request.page_size,
                    request.token.clone(),
                )
                .await
                .map_err(|e| Status::invalid_argument(e.to_string()))?,
        ))
    }

    async fn set_pairs(
        &self,
        request: tonic::Request<SetPairsRequest>,
    ) -> Result<tonic::Response<()>, tonic::Status> {
        let request = request.get_ref();
        self.database
            .set(request.pairs.clone())
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(()))
    }

    async fn get_pair(
        &self,
        request: tonic::Request<PairRequest>,
    ) -> Result<tonic::Response<proto_kv_service::Pair>, tonic::Status> {
        let request = request.get_ref();
        match self
            .database
            .get(&request.project_id, &request.namespace, &request.key)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
        {
            Some(v) => Ok(Response::new(v)),
            None => Err(Status::not_found(format!(
                "'{}' was not found in '{}' namespace.",
                request.key, request.namespace
            ))),
        }
    }

    async fn delete_pairs(
        &self,
        request: tonic::Request<DeletePairsRequest>,
    ) -> Result<tonic::Response<()>, tonic::Status> {
        let request = request.get_ref();
        self.database
            .delete(
                &request.project_id,
                &request.namespace,
                request.keys.clone(),
            )
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(()))
    }
}
