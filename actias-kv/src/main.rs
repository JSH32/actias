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
mod migrate;

pub mod proto_kv_service {
    tonic::include_proto!("kv_service");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    setup_tracing().expect("tracing subscriber could not be installed");

    let config = Config::new();

    // The same image is both the service and its migrator, so deployments
    // never carry a second binary that can drift from the schema it applies.
    if std::env::args().any(|arg| arg == "--migrate") {
        info!("Applying kv migrations");
        migrate::run(config.scylla_nodes).await?;
        info!("Migrations applied");
        return Ok(());
    }

    let addr = format!("0.0.0.0:{}", config.port).parse()?;
    let database = Database::new(database::connect(config.scylla_nodes).await).await;

    info!("KV Service listening on {}", addr);

    Server::builder()
        .add_service(KvServiceServer::new(KvService::new(database)))
        .serve(addr)
        .await?;

    Ok(())
}
