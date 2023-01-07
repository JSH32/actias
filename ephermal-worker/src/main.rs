mod config;
mod extensions;
mod runtime;
mod server;

use std::{convert::Infallible, net::SocketAddr};

use ephermal_common::{
    setup_tracing,
    tracing::{error, info},
};
use hyper::{
    service::{make_service_fn, service_fn},
    Server,
};

use server::http_handler;

use crate::config::Config;
use crate::script_service::script_service_client::ScriptServiceClient;

pub mod script_service {
    tonic::include_proto!("script_service");
}

#[tokio::main]
pub async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Setup tracing.
    setup_tracing().unwrap();

    let config = Config::new();
    let script_client = ScriptServiceClient::connect(config.script_service_uri).await?;

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    let make_svc = make_service_fn(|_conn| {
        let client = script_client.clone();
        async { Ok::<_, Infallible>(service_fn(move |req| http_handler(req, client.clone()))) }
    });

    let server = Server::bind(&addr).serve(make_svc);

    info!("Serving on {}", addr.to_string());

    Ok(if let Err(e) = server.await {
        error!("Server error: {}", e);
    })
}
