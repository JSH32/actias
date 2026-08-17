mod config;
mod extensions;
mod runtime;
mod server;

use std::net::SocketAddr;

use actias_common::{setup_tracing, tracing::info};

use crate::proto::script_service::script_service_client::ScriptServiceClient;
use crate::{config::Config, proto::kv_service::kv_service_client::KvServiceClient};

pub mod proto {
    pub mod bundle {
        tonic::include_proto!("bundle");
    }

    pub mod script_service {
        tonic::include_proto!("script_service");
    }

    pub mod kv_service {
        tonic::include_proto!("kv_service");
    }
}

/// Resolves when the process is asked to stop, so in-flight scripts finish
/// instead of dying mid-request on deploy.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("ctrl-c handler could not be installed");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("sigterm handler could not be installed")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("Shutting down");
}

#[tokio::main]
pub async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    setup_tracing().expect("tracing subscriber could not be installed");

    let config = Config::new();
    let script_client = ScriptServiceClient::connect(config.script_service_uri).await?;
    let kv_client = KvServiceClient::connect(config.kv_service_uri).await?;

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    let app = server::router(
        server::Clients {
            script: script_client,
            kv: kv_client,
        },
        server::WorkerCaches::new(
            std::time::Duration::from_secs(config.pointer_ttl_secs),
            config.revision_cache_bytes,
        ),
        config.max_body_bytes,
        std::time::Duration::from_secs(config.request_timeout_secs),
    );

    info!("Serving on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}
