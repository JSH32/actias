mod config;
mod extensions;
mod runtime;
mod server;

use std::{convert::Infallible, net::SocketAddr};

use actias_common::{
    setup_tracing,
    tracing::{error, info},
};
use hyper::{
    service::{make_service_fn, service_fn},
    Server,
};

use server::http_handler;

use crate::config::Config;
use crate::proto::script_service::script_service_client::ScriptServiceClient;

pub mod proto {
    pub mod bundle {
        tonic::include_proto!("bundle");
    }

    pub mod script_service {
        tonic::include_proto!("script_service");
    }
}

#[tokio::main]
pub async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Setup tracing.
    setup_tracing().unwrap();

    let config = Config::new();
    let script_client = ScriptServiceClient::connect(config.script_service_uri).await?;

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
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
