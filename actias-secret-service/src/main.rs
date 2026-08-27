use actias_common::setup_tracing;
use actias_common::tracing::info;
use sea_orm::Database;
use tonic::transport::Server;

use crate::config::Config;
use crate::envelope::Envelope;
use crate::proto_secret_service::secret_service_server::SecretServiceServer;
use crate::secret_service::SecretService;

mod config;
mod entity;
mod envelope;
mod secret_service;

pub mod proto_secret_service {
    tonic::include_proto!("secret_service");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    setup_tracing().unwrap();

    let config = Config::new();
    let addr = format!("0.0.0.0:{}", config.port).parse().unwrap();

    info!("Secret Service listening on {}", addr);

    let database = Database::connect(&config.database_url).await?;
    let envelope = Envelope::new(
        config.master_key_id,
        config.master_key,
        config.previous_master,
    );

    // The standard grpc health service, the probe target: overall
    // status flips to serving once everything above constructed.
    let (health_reporter, health_service) = tonic_health::server::health_reporter();
    health_reporter
        .set_service_status("", tonic_health::ServingStatus::Serving)
        .await;

    Server::builder()
        .layer(actias_common::otel::TraceExtract)
        .add_service(health_service)
        .add_service(SecretServiceServer::new(SecretService::new(
            database, envelope,
        )))
        .serve(addr)
        .await?;

    Ok(())
}
