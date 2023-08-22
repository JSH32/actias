use crate::database::Database;
use crate::kv_service::KvService;
use crate::proto_kv_service::kv_service_server::KvServiceServer;
use actias_common::setup_tracing;
use actias_common::tracing::info;
use tonic::transport::Server;

use crate::config::Config;

mod config;
mod database;
mod kv_service;

pub mod proto_kv_service {
    tonic::include_proto!("kv_service");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    setup_tracing().unwrap();

    let config = Config::new();
    let addr = format!("[::1]:{}", config.port).parse().unwrap();

    let database = Database::new(config.scylla_nodes).await;

    info!("KV Service listening on {}", addr);

    Server::builder()
        .add_service(KvServiceServer::new(KvService::new(database)))
        .serve(addr)
        .await?;

    Ok(())
}
