//! The placement service binary: one per region, the store every worker
//! claims objects in, heartbeats into and sweeps alarms from, over
//! postgres or scylla. Also its own migrator, under `--migrate`.

use std::sync::Arc;

use crate::config::{Backend, Config};
use actias_common::setup_tracing;
use actias_common::tracing::info;
use actias_placement::proto_node_registry::node_registry_service_server::NodeRegistryServiceServer;
use actias_placement::registry;
use actias_placement::store::PlacementStore;
use tonic::transport::Server;

mod config;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    setup_tracing().expect("tracing subscriber could not be installed");

    let config = Config::new();

    // The same image is both the service and its migrator, so a
    // deployment never carries a second artifact that can drift from
    // the schema it applies.
    if std::env::args().any(|arg| arg == "--migrate") {
        info!("Applying placement migrations");
        match config.backend {
            Backend::Postgres(url) => {
                actias_common::postgres::ensure_database(&url).await?;
                let pool = actias_placement::postgres::connect(&url).await;
                sqlx::migrate!("./migrations").run(&pool).await?;
            }
            Backend::Scylla {
                nodes,
                dc,
                replication_factor,
            } => actias_placement::migrate::run(nodes, &dc, replication_factor).await?,
        }
        info!("Migrations applied");
        return Ok(());
    }

    let addr = format!("0.0.0.0:{}", config.port).parse()?;
    let store: Arc<dyn PlacementStore> = match &config.backend {
        Backend::Postgres(url) => Arc::new(actias_placement::postgres::PostgresStore::new(
            actias_placement::postgres::connect(url).await,
        )),
        Backend::Scylla { nodes, .. } => Arc::new(
            actias_placement::scylla::ScyllaStore::new(
                actias_placement::scylla::connect(nodes.clone()).await,
                config.region.clone(),
            )
            .await,
        ),
    };

    // The standard grpc health service, the probe target.
    let (health_reporter, health_service) = tonic_health::server::health_reporter();
    health_reporter
        .set_service_status("", tonic_health::ServingStatus::Serving)
        .await;

    // The age-out sweep on its own timer: several beats per ttl, so a
    // dead node's rows go without any claim paying for the delete.
    let sweeper = registry::NodeRegistry::new(store.clone(), config.node_ttl_secs);
    tokio::spawn(async move {
        let period = std::time::Duration::from_secs((config.node_ttl_secs / 3).max(1) as u64);
        loop {
            tokio::time::sleep(period).await;
            if let Err(error) = sweeper.reap_expired().await {
                actias_common::tracing::warn!(%error, "node age-out sweep failed");
            }
        }
    });

    info!("Placement service listening on {}", addr);

    Server::builder()
        .layer(actias_common::otel::TraceExtract)
        .add_service(health_service)
        .add_service(NodeRegistryServiceServer::new(registry::NodeRegistry::new(
            store,
            config.node_ttl_secs,
        )))
        .serve(addr)
        .await?;

    Ok(())
}
