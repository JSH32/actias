use crate::{
    database::Database,
    proto_kv_service::{
        self, kv_service_server, ListNamespacesRequest, ListNamespacesResponse, ListPairsRequest,
        ListPairsResponse, PairRequest,
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
        todo!();
    }

    async fn list_pairs(
        &self,
        request: tonic::Request<ListPairsRequest>,
    ) -> Result<tonic::Response<ListPairsResponse>, tonic::Status> {
        todo!()
    }

    async fn create_pair(
        &self,
        request: tonic::Request<proto_kv_service::Pair>,
    ) -> Result<tonic::Response<()>, tonic::Status> {
        todo!()
    }
    async fn get_pair(
        &self,
        request: tonic::Request<PairRequest>,
    ) -> Result<tonic::Response<proto_kv_service::Pair>, tonic::Status> {
        todo!()
    }

    async fn delete_pair(
        &self,
        request: tonic::Request<PairRequest>,
    ) -> Result<tonic::Response<()>, tonic::Status> {
        todo!()
    }
}
