mod extensions;
mod runtime;
mod server;

use std::{convert::Infallible, net::SocketAddr};

use hyper::{
    service::{make_service_fn, service_fn},
    Server,
};
use server::http_handler;
use tracing::{error, info, Level};
use tracing_subscriber::FmtSubscriber;

#[tokio::main]
pub async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Setup tracing.
    tracing::subscriber::set_global_default(
        FmtSubscriber::builder()
            .with_max_level(Level::TRACE)
            .finish(),
    )
    .unwrap();

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    let make_svc = make_service_fn(|_conn| async { Ok::<_, Infallible>(service_fn(http_handler)) });
    let server = Server::bind(&addr).serve(make_svc);

    info!("Serving on {}", addr.to_string());

    Ok(if let Err(e) = server.await {
        error!("Server error: {}", e);
    })
}
