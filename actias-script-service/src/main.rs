//! The script service binary: the control plane's record of projects,
//! scripts, revisions and bundles, plus the node registry workers
//! heartbeat into. Also its own migrator, under `--migrate`.

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

    // The same image is both the service and its migrator, so a
    // deployment never carries a second artifact that can drift from
    // the schema it applies: these are the migrations compiled into
    // this binary. It reads DATABASE_URL and nothing else, ahead of
    // the service's own config, so applying a schema never requires
    // the credentials serving traffic does.
    if std::env::args().any(|arg| arg == "--migrate") {
        let url: String = actias_common::config::get_env("DATABASE_URL");
        info!("Applying script service migrations");
        actias_common::postgres::ensure_database(&url).await?;
        let pool = PgPoolOptions::new().connect(&url).await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        info!("Migrations applied");
        return Ok(());
    }

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

    // The standard grpc health service, the probe target: overall
    // status flips to serving once everything above constructed.
    let (health_reporter, health_service) = tonic_health::server::health_reporter();
    health_reporter
        .set_service_status("", tonic_health::ServingStatus::Serving)
        .await;

    // The age-out sweep on its own timer: several beats per ttl, so a
    // dead node's rows go without any claim paying for the delete.
    let sweeper = node_registry::NodeRegistry::new(pool.clone(), config.node_ttl_secs);
    tokio::spawn(async move {
        let period = std::time::Duration::from_secs((config.node_ttl_secs / 3).max(1) as u64);
        loop {
            tokio::time::sleep(period).await;
            if let Err(error) = sweeper.reap_expired().await {
                actias_common::tracing::warn!(%error, "node age-out sweep failed");
            }
        }
    });

    Server::builder()
        .layer(actias_common::otel::TraceExtract)
        .add_service(health_service)
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
