use crate::proto_script_service::script_service_server::ScriptServiceServer;
use crate::{config::Config, script_service::ScriptService};
use ephermal_common::setup_tracing;
use ephermal_common::tracing::info;
use sqlx::postgres::PgPoolOptions;
use tonic::transport::Server;

mod config;
mod script_service;

pub mod bundle {
    tonic::include_proto!("bundle");
}

pub mod proto_script_service {
    tonic::include_proto!("script_service");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    setup_tracing().unwrap();

    let config = Config::new();
    let addr = format!("[::1]:{}", config.port).parse().unwrap();

    info!("Script Service listening on {}", addr);

    let pool = PgPoolOptions::new().connect(&config.database_url).await?;

    Server::builder()
        .add_service(ScriptServiceServer::new(ScriptService::new(pool)))
        .serve(addr)
        .await?;

    Ok(())
}
