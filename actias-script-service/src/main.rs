use crate::live_script::LiveScriptManager;
use crate::proto_node_registry::node_registry_service_server::NodeRegistryServiceServer;
use crate::proto_script_service::script_service_server::ScriptServiceServer;
use crate::{config::Config, script_service::ScriptService};
use actias_common::setup_tracing;
use actias_common::tracing::info;
use sqlx::postgres::PgPoolOptions;
use tonic::transport::Server;

mod blob_store;
mod config;
mod database_types;
mod live_script;
mod node_registry;
mod script_service;
mod util;

pub mod bundle {
    tonic::include_proto!("bundle");
}

pub mod proto_script_service {
    tonic::include_proto!("script_service");
}

pub mod proto_node_registry {
    tonic::include_proto!("node_registry");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    setup_tracing().unwrap();

    let config = Config::new();
    let addr = format!("0.0.0.0:{}", config.port).parse().unwrap();

    info!("Script Service listening on {}", addr);

    let pool = PgPoolOptions::new().connect(&config.database_url).await?;
    let live_script_manager = LiveScriptManager::new(&config.redis_url);

    let blobs = blob_store::BlobStore::new(blob_store::BlobStoreConfig {
        endpoint: config.s3_endpoint,
        access_key: config.s3_access_key,
        secret_key: config.s3_secret_key,
        bucket: config.s3_bucket,
    })
    .await;

    Server::builder()
        .add_service(ScriptServiceServer::new(ScriptService::new(
            pool.clone(),
            live_script_manager,
            blobs,
        )))
        .add_service(NodeRegistryServiceServer::new(
            node_registry::NodeRegistry::new(pool, config.node_ttl_secs),
        ))
        .serve(addr)
        .await?;

    Ok(())
}
