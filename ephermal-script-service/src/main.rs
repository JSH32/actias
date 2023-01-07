use crate::proto_script_service::script_service_server::ScriptServiceServer;
use crate::{config::Config, script_service::ScriptService};
use ephermal_common::setup_tracing;
use ephermal_common::tracing::info;
use mongodb::{options::ClientOptions, Client};
use tonic::transport::Server;

mod config;
mod script_service;

pub mod proto_script_service {
    tonic::include_proto!("script_service");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    setup_tracing().unwrap();

    let config = Config::new();
    let addr = format!("[::1]:{}", config.port).parse().unwrap();

    info!("Script Service listening on {}", addr);

    let mut client_options = ClientOptions::parse(config.mongo_uri).await?;
    client_options.app_name = Some("script_service".to_string());
    let client = Client::with_options(client_options)?;
    let database = client.database("ephermal_script_service");

    Server::builder()
        .add_service(ScriptServiceServer::new(
            ScriptService::new(&database).await,
        ))
        .serve(addr)
        .await?;

    Ok(())
}
