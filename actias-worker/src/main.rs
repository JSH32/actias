mod blob_cache;
mod config;
mod heartbeat;
mod server;
mod sweeper;

use std::net::SocketAddr;

use actias_common::{setup_tracing, tracing::info};

use crate::config::Config;
use actias_worker_core::proto::kv_service::kv_service_client::KvServiceClient;
use actias_worker_core::proto::script_service::script_service_client::ScriptServiceClient;

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

    // The worker's own backends are denied to scripts by name as well as by
    // address, so the policy holds even where the services resolve publicly.
    let mut denied_hosts = config.egress_denied_hosts.clone();
    for uri in [&config.script_service_uri, &config.kv_service_uri] {
        if let Ok(uri) = uri.parse::<axum::http::Uri>()
            && let Some(host) = uri.host()
        {
            denied_hosts.push(host.to_owned());
        }
    }
    let egress = actias_worker_core::egress::EgressClient::new(
        actias_worker_core::egress::EgressPolicy::new(denied_hosts, config.egress_allow_private),
    )?;

    let script_client = ScriptServiceClient::connect(config.script_service_uri.clone()).await?;
    let kv_client = KvServiceClient::connect(config.kv_service_uri).await?;

    // The registry rides in the script-service binary, so it answers on the
    // same channel. Membership is not on the request path: the loop retries
    // forever and the worker serves regardless.
    let registry_client =
        actias_worker_core::proto::node_registry::node_registry_service_client::NodeRegistryServiceClient::connect(
            config.script_service_uri.clone(),
        )
        .await?;
    let in_flight = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let node_identity = std::sync::Arc::new(std::sync::RwLock::new(None));
    tokio::spawn(heartbeat::register_and_heartbeat(
        registry_client.clone(),
        config.node_address.clone(),
        in_flight.clone(),
        node_identity.clone(),
    ));

    let redis = redis::aio::ConnectionManager::new(
        redis::Client::open(config.redis_url).expect("REDIS_URL is not a valid redis url"),
    )
    .await?;

    use base64::Engine;
    let secrets_key = config.secret_encryption_key.map(|encoded| {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .expect("SECRET_ENCRYPTION_KEY is not valid base64");
        let key: [u8; actias_worker_core::extensions::secrets::KEY_LEN] = bytes
            .try_into()
            .expect("SECRET_ENCRYPTION_KEY must decode to exactly 32 bytes");
        std::sync::Arc::new(key)
    });

    // Object storage must exist before the first object call needs it.
    std::fs::create_dir_all(&config.object_data_dir)?;

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    let state = server::AppState {
        clients: server::Clients {
            script: script_client,
            kv: kv_client,
        },
        caches: server::WorkerCaches::new(
            std::time::Duration::from_secs(config.pointer_ttl_secs),
            config.revision_cache_bytes,
        ),
        blobs: blob_cache::BlobCache::new(blob_cache::BlobCacheConfig {
            endpoint: config.s3_endpoint,
            access_key: config.s3_access_key,
            secret_key: config.s3_secret_key,
            bucket: config.s3_bucket,
            cache_bytes: config.blob_cache_bytes,
        }),
        egress,
        redis: Some(redis),
        secrets_key,
        request_timeout: std::time::Duration::from_secs(config.request_timeout_secs),
        in_flight,
        objects: std::sync::Arc::new(actias_worker_core::objects::ObjectHost::default()),
        object_data_dir: std::path::PathBuf::from(config.object_data_dir),
        object_db_max_bytes: config.object_db_max_bytes,
        object_idle_after: std::time::Duration::from_secs(config.object_idle_secs),
        node_identity,
        registry: registry_client,
        base_domain: config.base_domain,
    };

    // Due alarms in cold files fire without anyone asking; the sweep is
    // what makes hibernation and crashes indistinguishable to an alarm.
    tokio::spawn(sweeper::run(
        state.clone(),
        std::time::Duration::from_secs(config.object_sweep_secs),
    ));

    let app = server::router(state, config.max_body_bytes);

    info!("Serving on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}
