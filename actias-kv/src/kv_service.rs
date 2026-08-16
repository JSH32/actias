use actias_common::tracing::error;
use tonic::{Response, Status};

use crate::{
    database::{Database, DatabaseError},
    proto_kv_service::{
        self, CreateNamespaceRequest, DeleteNamespaceRequest, DeletePairsRequest,
        DeleteProjectRequest, ListNamespacesRequest, ListNamespacesResponse, ListPairsRequest,
        ListPairsResponse, Namespace, PairRequest, SetPairsRequest, kv_service_server,
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

/// The one place a [`DatabaseError`] becomes a wire status.
///
/// Invalid input is the caller's to see; everything else is logged here and
/// leaves as a bare internal error, because driver messages quote hosts and
/// queries.
impl From<DatabaseError> for Status {
    fn from(err: DatabaseError) -> Self {
        match err {
            DatabaseError::Invalid(e) => Status::invalid_argument(e),
            other => {
                error!(error = %other, "kv database error");
                Status::internal("Storage error.")
            }
        }
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
            self.database.get_namespaces(&request.project_id).await?,
        ))
    }

    async fn create_namespace(
        &self,
        request: tonic::Request<CreateNamespaceRequest>,
    ) -> Result<tonic::Response<Namespace>, tonic::Status> {
        let request = request.get_ref();

        self.database
            .create_namespace(&request.project_id, &request.namespace)
            .await?;

        Ok(Response::new(Namespace {
            project_id: request.project_id.clone(),
            name: request.namespace.clone(),
            count: 0,
        }))
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
                .await?,
        ))
    }

    async fn set_pairs(
        &self,
        request: tonic::Request<SetPairsRequest>,
    ) -> Result<tonic::Response<()>, tonic::Status> {
        let request = request.get_ref();
        self.database.set(request.pairs.clone()).await?;

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
            .await?
        {
            Some(v) => Ok(Response::new(v)),
            None => Err(Status::not_found(format!(
                "'{}' was not found in '{}' namespace.",
                request.key, request.namespace
            ))),
        }
    }

    async fn delete_project(
        &self,
        request: tonic::Request<DeleteProjectRequest>,
    ) -> Result<tonic::Response<()>, tonic::Status> {
        let request = request.get_ref();
        self.database.delete_project(&request.project_id).await?;

        Ok(Response::new(()))
    }

    async fn delete_namespace(
        &self,
        request: tonic::Request<DeleteNamespaceRequest>,
    ) -> Result<tonic::Response<()>, tonic::Status> {
        let request = request.get_ref();

        self.database
            .delete_namespace(&request.project_id, &request.namespace)
            .await?;

        Ok(Response::new(()))
    }

    async fn delete_pairs(
        &self,
        request: tonic::Request<DeletePairsRequest>,
    ) -> Result<tonic::Response<()>, tonic::Status> {
        let request = request.get_ref();

        self.database.delete(request.pairs.clone()).await?;

        Ok(Response::new(()))
    }
}
