//! The script service binary: the control plane's record of projects,
//! scripts, revisions, bundles and project policy. Also its own
//! migrator, under `--migrate`.

use crate::live_script::LiveScriptManager;
use crate::proto_node_registry::node_registry_service_client::NodeRegistryServiceClient;
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
mod mover;
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
    // Worker-facing reads go to a regional replica when there is one.
    let reads = match &config.read_database_url {
        Some(url) => {
            info!("worker-facing reads use the read replica");
            PgPoolOptions::new()
                .max_connections(20)
                .connect(url)
                .await?
        }
        None => pool.clone(),
    };
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

    // The placement service, for the instance directory's orphan
    // fallback; lazy, so booting never waits on it.
    let placement = NodeRegistryServiceClient::new(
        tonic::transport::Endpoint::from_shared(config.placement_service_uri.clone())?
            .connect_lazy(),
    );

    Server::builder()
        .layer(actias_common::otel::TraceExtract)
        .add_service(health_service)
        .add_service(ScriptServiceServer::new(ScriptService::new(
            script_service::Databases {
                writes: pool.clone(),
                reads,
            },
            live_script_manager,
            blobs,
            placement,
            config.region.clone(),
            config.placement_service_uri.clone(),
            std::time::Duration::from_secs(config.move_drain_secs),
        )))
        .serve(addr)
        .await?;

    Ok(())
}
