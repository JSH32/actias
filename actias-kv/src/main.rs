//! The kv service binary: namespaced key-value pairs with typed values
//! and optional expiry, over postgres or scylla.

use std::sync::Arc;

use crate::config::{Backend, Config};
use crate::kv_service::KvService;
use crate::proto_kv_service::kv_service_server::KvServiceServer;
use crate::store::KvStore;
use actias_common::setup_tracing;
use actias_common::tracing::{info, warn};
use tonic::transport::Server;

mod config;
mod kv_service;
mod migrate;
mod postgres_store;
mod scylla_store;
mod store;

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
        match config.backend {
            Backend::Postgres(url) => {
                actias_common::postgres::ensure_database(&url).await?;
                let pool = postgres_store::connect(&url).await;
                migrate::apply_postgres(&pool).await?;
            }
            Backend::Scylla(nodes) => migrate::run(nodes).await?,
        }
        info!("Migrations applied");
        return Ok(());
    }

    let addr = format!("0.0.0.0:{}", config.port).parse()?;
    let store: Arc<dyn KvStore> = match &config.backend {
        Backend::Postgres(url) => {
            let store = Arc::new(postgres_store::PostgresStore::new(
                postgres_store::connect(url).await,
            ));

            // Expiry is enforced at read time; the sweeper only reclaims
            // space, so its failures warn and its cadence is tunable.
            let sweeper = store.clone();
            let every = std::time::Duration::from_secs(config.sweep_secs.max(1));
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(every).await;
                    loop {
                        match sweeper.sweep(1000).await {
                            Ok(0) => break,
                            Ok(_) => continue,
                            Err(error) => {
                                warn!(%error, "expired-pair sweep failed");
                                break;
                            }
                        }
                    }
                }
            });

            store
        }
        Backend::Scylla(nodes) => Arc::new(
            scylla_store::ScyllaStore::new(scylla_store::connect(nodes.clone()).await).await,
        ),
    };

    info!("KV Service listening on {}", addr);

    // The standard grpc health service, the probe target: overall
    // status flips to serving once the store connected.
    let (health_reporter, health_service) = tonic_health::server::health_reporter();
    health_reporter
        .set_service_status("", tonic_health::ServingStatus::Serving)
        .await;

    Server::builder()
        .layer(actias_common::otel::TraceExtract)
        .add_service(health_service)
        .add_service(KvServiceServer::new(KvService::new(store)))
        .serve(addr)
        .await?;

    Ok(())
}
