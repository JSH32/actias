mod blob_cache;
mod config;
mod data_plane;
mod heartbeat;
mod metrics;
mod object_store;
mod routing;
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

    // Every platform client carries the trace-inject interceptor; it is
    // a no-op until otel is configured (actias_common::otel).
    let inject: actias_worker_core::GrpcInterceptor = actias_common::otel::trace_inject;
    let script_channel =
        tonic::transport::Endpoint::from_shared(config.script_service_uri.clone())?
            .connect()
            .await?;
    let script_client = ScriptServiceClient::with_interceptor(script_channel.clone(), inject);
    let kv_channel = tonic::transport::Endpoint::from_shared(config.kv_service_uri)?
        .connect()
        .await?;
    let kv_client = KvServiceClient::with_interceptor(kv_channel, inject);

    // The registry rides in the script-service binary, so it answers on the
    // same channel. Membership is not on the request path: the loop retries
    // forever and the worker serves regardless.
    let registry_client =
        actias_worker_core::proto::node_registry::node_registry_service_client::NodeRegistryServiceClient::with_interceptor(
            script_channel,
            inject,
        );
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

    let secret_client = match config.secret_service_uri {
        Some(uri) => {
            let channel = tonic::transport::Endpoint::from_shared(uri)?
                .connect()
                .await?;
            Some(
                actias_worker_core::proto::secret_service::secret_service_client::SecretServiceClient::with_interceptor(
                    channel, inject,
                ),
            )
        }
        None => None,
    };

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
            endpoint: config.s3_endpoint.clone(),
            access_key: config.s3_access_key.clone(),
            secret_key: config.s3_secret_key.clone(),
            bucket: config.s3_bucket.clone(),
            cache_bytes: config.blob_cache_bytes,
        }),
        replica_ttl: std::time::Duration::from_secs(config.replica_ttl_secs),
        peers: moka::future::Cache::new(100),
        internal_token: config.internal_token,
        object_store: std::sync::Arc::new(object_store::ObjectStore::new(
            blob_cache::s3_client(
                &config.s3_endpoint,
                &config.s3_access_key,
                &config.s3_secret_key,
            ),
            config.s3_bucket,
        )),
        egress,
        redis: Some(redis),
        secret_client,
        request_timeout: std::time::Duration::from_secs(config.request_timeout_secs),
        in_flight,
        objects: std::sync::Arc::new(actias_worker_core::objects::ObjectHost::default()),
        metrics: std::sync::Arc::default(),
        armed_crons: std::sync::Arc::default(),
        object_data_dir: std::path::PathBuf::from(config.object_data_dir),
        object_db_max_bytes: config.object_db_max_bytes,
        object_idle_after: std::time::Duration::from_secs(config.object_idle_secs),
        queue_policy: actias_worker_core::platform::queue::QueuePolicy {
            max_attempts: config.queue_max_attempts,
            backoff_base_ms: config.queue_backoff_base_ms,
        },
        node_identity,
        registry: registry_client,
        base_domain: config.base_domain,
        connections: std::sync::Arc::default(),
    };

    // Due alarms in cold files fire without anyone asking; the sweep is
    // what makes hibernation and crashes indistinguishable to an alarm.
    tokio::spawn(sweeper::run(
        state.clone(),
        std::time::Duration::from_secs(config.object_sweep_secs),
    ));

    // The data plane: object dispatch and typed reads, cluster-internal.
    // The registry address other nodes and the api dial is THIS listener.
    let grpc_addr = SocketAddr::from(([0, 0, 0, 0], config.grpc_port));
    // The standard grpc health service rides the data plane unguarded
    // (health is not a capability), the probe target for this node.
    let (health_reporter, health_service) = tonic_health::server::health_reporter();
    health_reporter
        .set_service_status("", tonic_health::ServingStatus::Serving)
        .await;
    let data_plane = tonic::transport::Server::builder()
        .layer(actias_common::otel::TraceExtract)
        .add_service(health_service)
        .add_service(
            actias_worker_core::proto::worker_data::worker_data_server::WorkerDataServer::with_interceptor(
                data_plane::WorkerDataService::new(state.clone()),
                data_plane::require_internal_token(state.internal_token.clone()),
            ),
        )
        .serve_with_shutdown(grpc_addr, shutdown_signal());

    let app = server::router(state.clone(), config.max_body_bytes)
        .layer(actias_common::otel::TraceExtract);

    info!("Serving http on {addr}, data plane on {grpc_addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    let http = axum::serve(listener, app).with_graceful_shutdown(shutdown_signal());

    // The goodbye fires AT the shutdown signal, not after the drain:
    // graceful drain waits on peers' persistent h2 channels and can
    // outlive the sigkill window, and the node stops being routable the
    // moment it stops accepting anyway. Deregistering frees this node's
    // leases at once, so a deploy's replacement claims them immediately
    // instead of serving a ttl's worth of dead forwards. Best effort; a
    // crash still ages out, and the epoch fence covers the drain window.
    let registry_for_goodbye = state.registry.clone();
    let identity_for_goodbye = state.node_identity.clone();
    let goodbye_task = tokio::spawn(async move {
        shutdown_signal().await;
        let node_id = identity_for_goodbye
            .read()
            .expect("no poisoned lock")
            .clone();
        let Some(node_id) = node_id else { return };
        let mut registry = registry_for_goodbye;
        let goodbye = registry.deregister(
            actias_worker_core::proto::node_registry::DeregisterRequest {
                node_id: node_id.clone(),
            },
        );
        match tokio::time::timeout(std::time::Duration::from_secs(5), goodbye).await {
            Ok(Ok(_)) => info!(node_id, "deregistered from the placement store"),
            Ok(Err(error)) => {
                actias_common::tracing::warn!(%error, "deregistration failed; age-out covers it")
            }
            Err(_) => {
                actias_common::tracing::warn!("deregistration timed out; age-out covers it")
            }
        }
    });

    // Either listener failing takes the process down: a worker whose data
    // plane is dark would strand every object homed on it.
    tokio::try_join!(async { http.await.map_err(anyhow::Error::from) }, async {
        data_plane.await.map_err(anyhow::Error::from)
    },)?;

    // A fast drain must not outrun the goodbye: when nothing holds the
    // listeners open, main gets here in milliseconds and exiting now
    // would kill the spawned deregistration mid-flight, silently. Its
    // own 5s rpc timeout bounds this wait; the extra second is slack.
    let _ = tokio::time::timeout(std::time::Duration::from_secs(6), goodbye_task).await;

    Ok(())
}
