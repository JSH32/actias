//! The secret service binary: envelope-encrypted project secrets, with
//! the active master key wrapping every new write. Also its own
//! migrator, under `--migrate`.

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

    // The same image is both the service and its migrator, so a
    // deployment never carries a second artifact that can drift from
    // the schema it applies: these are the migrations compiled into
    // this binary. It reads DATABASE_URL and nothing else, ahead of
    // the service's own config, so applying a schema never requires
    // the master key serving traffic does.
    if std::env::args().any(|arg| arg == "--migrate") {
        let url: String = actias_common::config::get_env("DATABASE_URL");
        info!("Applying secret service migrations");
        actias_common::postgres::ensure_database(&url).await?;
        let pool = sqlx::postgres::PgPoolOptions::new().connect(&url).await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        info!("Migrations applied");
        return Ok(());
    }

    let config = Config::new();
    let addr = format!("0.0.0.0:{}", config.port).parse().unwrap();

    info!("Secret Service listening on {}", addr);

    let database = Database::connect(&config.database_url).await?;
    // A worker's resolve reads from a regional replica when there is one.
    let reads = match &config.read_database_url {
        Some(url) => sea_orm::Database::connect(url).await?,
        None => database.clone(),
    };

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
            database, reads, envelope,
        )))
        .serve(addr)
        .await?;

    Ok(())
}
