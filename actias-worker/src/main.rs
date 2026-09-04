//! The worker binary: the http surface scripts are served from, the
//! grpc data plane peers and the api call, and the loops that keep an
//! object's durable state, its index and its placement current.

mod blob_cache;
mod config;
mod data_plane;
mod directory;
mod heartbeat;
mod metrics;
mod objects;
mod routing;
mod server;
mod shell_run;
mod vm_pool;

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
    for uri in [
        &config.script_service_uri,
        &config.kv_service_uri,
        &config.placement_service_uri,
    ] {
        if let Ok(uri) = uri.parse::<axum::http::Uri>()
            && let Some(host) = uri.host()
        {
            denied_hosts.push(host.to_owned());
        }
    }
    let egress = actias_worker_core::egress::EgressClient::new(
        actias_worker_core::egress::EgressPolicy::new(denied_hosts, config.egress_allow_private),
    )?;

    // Every platform client rides the traced channel: each call gets a
    // client span and carries its context (a no-op until otel is
    // configured, actias_common::otel).
    let script_channel =
        tonic::transport::Endpoint::from_shared(config.script_service_uri.clone())?
            .connect()
            .await?;
    let script_client =
        ScriptServiceClient::new(actias_worker_core::plain_grpc(script_channel.clone()));
    let kv_channel = tonic::transport::Endpoint::from_shared(config.kv_service_uri)?
        .connect()
        .await?;
    let kv_client = KvServiceClient::new(actias_worker_core::plain_grpc(kv_channel));

    // The region's placement service. Membership is not on the request
    // path: the loop retries forever and the worker serves regardless.
    let placement_channel =
        tonic::transport::Endpoint::from_shared(config.placement_service_uri.clone())?
            .connect()
            .await?;
    let registry_client =
        actias_worker_core::proto::node_registry::node_registry_service_client::NodeRegistryServiceClient::new(
            actias_worker_core::plain_grpc(placement_channel),
        );
    drop(script_channel);
    let in_flight = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let node_identity = std::sync::Arc::new(std::sync::RwLock::new(None));
    let membership_generation = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    tokio::spawn(heartbeat::register_and_heartbeat(
        registry_client.clone(),
        config.node_address.clone(),
        in_flight.clone(),
        node_identity.clone(),
        membership_generation.clone(),
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
                actias_worker_core::proto::secret_service::secret_service_client::SecretServiceClient::new(
                    actias_worker_core::plain_grpc(channel),
                ),
            )
        }
        None => None,
    };

    // Object storage must exist before the first object call needs it.
    std::fs::create_dir_all(&config.object_data_dir)?;

    // The directory syncer needs the store before the state that holds
    // both, so it is built here and shared into the state and the flush
    // loop below.
    // The region's object bucket exists before anything ships to it; a
    // second region's bucket is created by its first worker.
    blob_cache::ensure_bucket(
        &blob_cache::s3_client(
            &config.s3_endpoint,
            &config.s3_access_key,
            &config.s3_secret_key,
        ),
        &config.object_bucket,
    )
    .await;
    let directory_store = std::sync::Arc::new(objects::store::ObjectStore::new(
        blob_cache::s3_client(
            &config.s3_endpoint,
            &config.s3_access_key,
            &config.s3_secret_key,
        ),
        config.object_bucket.clone(),
        config.directory_cache_bytes,
        config.object_store_parallel,
    ));
    let directory_gauges: std::sync::Arc<directory::gauges::DirectoryGauges> =
        std::sync::Arc::default();
    let directory_sync = {
        let store = directory_store.clone();
        directory::sync::DirectorySyncer::new(
            std::sync::Arc::new(move |class: directory::sync::ClassKey, name, bytes| {
                let store = store.clone();
                Box::pin(async move {
                    store
                        .put_directory_delta(&class.scope_id, &class.class, &name, bytes)
                        .await
                })
            }),
            std::path::PathBuf::from(&config.object_data_dir),
            directory_gauges.clone(),
        )
    };

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    // Every node-wide bound, split fairly among the projects using it.
    let shares =
        actias_worker_core::shares::ScopeShares::new(actias_worker_core::shares::ScopeLimits {
            requests: config.request_concurrency,
            blocking: config.blocking_concurrency,
            ships: config.object_ship_concurrency,
            connections: config.connection_limit,
            residents: config.object_resident_limit,
            directory_queries: config.directory_query_concurrency,
            floor: config.share_floor,
        });

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
        membership_generation: membership_generation.clone(),
        replica_store: objects::replica::ReplicaStore::new(
            std::path::PathBuf::from(&config.object_data_dir).join("replicas-held"),
            objects::replica::SyncMode::parse(&config.object_replica_sync),
            std::time::Duration::from_secs(config.object_replica_idle_secs),
        ),
        replica_count: config.object_replicas,
        replica_quorum: config.object_quorum,
        replica_ack: std::time::Duration::from_millis(config.object_replica_ack_ms),
        vm_pool: vm_pool::VmPool::new(config.object_vm_pool),
        node_address: config.node_address.clone(),
        region: config.region.clone(),
        shares: shares.clone(),
        rates: actias_worker_core::shares::RateLimits::new(),
        ship_states: std::sync::Arc::default(),
        peers: moka::future::Cache::new(100),
        holders: moka::future::Cache::builder()
            .max_capacity(200_000)
            .time_to_live(std::time::Duration::from_secs(15))
            .build(),
        node_addrs: moka::future::Cache::builder()
            .max_capacity(1_000)
            .time_to_live(std::time::Duration::from_secs(120))
            .build(),
        internal_token: config.internal_token,
        object_store: std::sync::Arc::new(objects::store::ObjectStore::new(
            blob_cache::s3_client(
                &config.s3_endpoint,
                &config.s3_access_key,
                &config.s3_secret_key,
            ),
            config.object_bucket.clone(),
            config.directory_cache_bytes,
            config.object_store_parallel,
        )),
        egress,
        redis: Some(redis),
        secret_client,
        request_timeout: std::time::Duration::from_secs(config.request_timeout_secs),
        guest_limits: server::GuestLimits {
            work: config.guest_work_limit,
            wall_secs: config.guest_wall_secs,
        },
        in_flight,
        objects: std::sync::Arc::new(actias_worker_core::objects::ObjectHost::bounded(
            shares.residents.clone(),
        )),
        metrics: std::sync::Arc::default(),
        armed_crons: std::sync::Arc::default(),
        object_data_dir: std::path::PathBuf::from(config.object_data_dir),
        object_db_max_bytes: config.object_db_max_bytes,
        ship_thresholds: objects::store::ShipThresholds {
            rotate_bytes: config.object_wal_rotate_bytes,
            rotate_fraction: config.object_wal_rotate_fraction,
            max_segments: config.object_max_segments,
        },
        ack_gate: std::time::Duration::from_millis(config.object_ack_gate_ms),
        ship_gauges: std::sync::Arc::default(),
        ship_limits: objects::shipper::ShipLimits::new(
            shares.ships.clone(),
            config.object_ship_reserved,
        ),
        directory_sync: directory_sync.clone(),
        directory_gauges,
        reader_membership: moka::future::Cache::builder()
            .time_to_live(std::time::Duration::from_secs(10))
            .build(),
        directory_overlays: std::sync::Arc::default(),
        directory_recomputed: std::sync::Arc::default(),
        directory_eval_budget_ms: config.directory_eval_budget_ms,
        admit_refusals: moka::future::Cache::builder()
            .max_capacity(100_000)
            .time_to_live(std::time::Duration::from_secs(config.pointer_ttl_secs))
            .build(),
        object_idle_after: std::time::Duration::from_secs(config.object_idle_secs),
        queue_policy: actias_worker_core::platform::queue::QueuePolicy {
            max_attempts: config.queue_max_attempts,
            backoff_base_ms: config.queue_backoff_base_ms,
        },
        node_identity,
        connection_gauges: std::sync::Arc::default(),
        connection_hibernate_after: (config.connection_hibernate_secs > 0)
            .then(|| std::time::Duration::from_secs(config.connection_hibernate_secs)),
        shippers: std::sync::Arc::default(),
        registry: registry_client,
        base_domain: config.base_domain,
        connections: std::sync::Arc::default(),
    };

    // Due alarms in cold files fire without anyone asking; the sweep is
    // what makes hibernation and crashes indistinguishable to an alarm.
    tokio::spawn(objects::sweeper::run(
        state.clone(),
        std::time::Duration::from_secs(config.object_sweep_secs),
    ));

    // Settled directory rows leave on their own cadence, never on a
    // caller's. A flush that fails keeps its rows, so the next tick
    // retries them.
    tokio::spawn({
        let syncer = directory_sync.clone();
        let interval = std::time::Duration::from_millis(config.directory_flush_ms);
        async move {
            loop {
                tokio::time::sleep(interval).await;
                let _ = syncer.flush().await;
            }
        }
    });

    // Deltas become a base on their own cadence. Any node may compact
    // any class it has written to; the lease decides which one does.
    tokio::spawn(directory::compact::run(
        state.clone(),
        std::time::Duration::from_secs(config.directory_compact_secs),
    ));

    // Rows the write path cannot reach: an object that never writes
    // again never offers one, and an object that expires never offers
    // its tombstone. Both are recovered from metadata, so this reads
    // manifests and never opens an object file.
    tokio::spawn(directory::rebuild::run(
        state.clone(),
        std::time::Duration::from_secs(config.directory_rebuild_secs),
    ));

    // The event-driven half: a node that died may have settled a write
    // whose row never reached a delta. Scoped to what that node held,
    // so it costs the crash rather than the cluster.
    tokio::spawn(directory::sweep::run(
        state.clone(),
        std::time::Duration::from_secs(config.directory_sweep_secs),
    ));

    // Overlays are pure cache; without this a node keeps one file per
    // class it was ever asked about, forever.
    tokio::spawn(directory::read::evict(
        state.clone(),
        std::time::Duration::from_secs(60),
        std::time::Duration::from_secs(config.directory_overlay_ttl_secs),
    ));

    // The data plane: object dispatch and typed reads, cluster-internal.
    // The registry address other nodes and the api dial is this listener.
    let grpc_addr = SocketAddr::from(([0, 0, 0, 0], config.grpc_port));
    // The standard grpc health service rides the data plane unguarded
    // (health is not a capability), the probe target for this node.
    let (health_reporter, health_service) = tonic_health::server::health_reporter();
    health_reporter
        .set_service_status("", tonic_health::ServingStatus::Serving)
        .await;
    // Idle replica copies leave once the store covers them; a copy whose
    // owner went quiet asks the manifest.
    {
        let store = state.object_store.clone();
        let coverage: objects::replica::Coverage =
            std::sync::Arc::new(move |object_id, epoch, base| {
                let store = store.clone();
                Box::pin(async move {
                    match store.manifest(&object_id).await.ok().flatten() {
                        Some(m) if m.deleted => objects::replica::Cover::Forgotten,
                        Some(m) if (m.epoch, m.base) == (epoch, base) => {
                            objects::replica::Cover::Through(m.wal_len)
                        }
                        _ => objects::replica::Cover::Unknown,
                    }
                })
            });
        state.replica_store.start_sweep(coverage);
    }

    let data_plane = tonic::transport::Server::builder()
        .layer(actias_common::otel::TraceExtract)
        .add_service(health_service)
        .add_service(tonic::service::interceptor::InterceptedService::new(
            actias_worker_core::proto::worker_data::worker_data_server::WorkerDataServer::new(
                data_plane::WorkerDataService::new(state.clone()),
            )
            .max_decoding_message_size(data_plane::PEER_MESSAGE_BYTES)
            .max_encoding_message_size(data_plane::PEER_MESSAGE_BYTES),
            data_plane::require_internal_token(state.internal_token.clone()),
        ))
        .serve_with_shutdown(grpc_addr, shutdown_signal());

    let app = server::router(state.clone(), config.max_body_bytes)
        .layer(actias_common::otel::TraceExtract);

    info!("Serving http on {addr}, data plane on {grpc_addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    let http = axum::serve(listener, app).with_graceful_shutdown(shutdown_signal());

    // The goodbye fires at the shutdown signal, not after the drain:
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

    // The drain flushed every request; now flush every dirty snapshot so
    // a deploy never leaves state only this volume holds. The floor is
    // five seconds; a node holding a real backlog gets more, because
    // shipping is bounded and the backlog drains in waves.
    objects::shipper::flush_all(&state.shippers, std::time::Duration::from_secs(5)).await;

    // The shippers settled, so every row they carried is durable and
    // offered. Flushing the syncer now is what lets a graceful stop
    // leave the index current instead of one interval behind, which is
    // also what the crash sweep uses to tell a clean exit from a death.
    if !state.directory_sync.settled() {
        let _ = state.directory_sync.flush().await;
    }

    // A fast drain must not outrun the goodbye: when nothing holds the
    // listeners open, main gets here in milliseconds and exiting now
    // would kill the spawned deregistration mid-flight, silently. Its
    // own 5s rpc timeout bounds this wait; the extra second is slack.
    let _ = tokio::time::timeout(std::time::Duration::from_secs(6), goodbye_task).await;

    Ok(())
}
