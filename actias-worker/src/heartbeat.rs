//! The worker's side of the placement store: register at boot, then prove
//! liveness at the cadence the registry dictates, reporting the in-flight
//! request gauge as load. A NOT_FOUND heartbeat means this node aged out
//! (a long stall, a registry wipe); the loop registers again rather than
//! dying, so membership self-heals.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use actias_common::tracing::{info, warn};
use actias_worker_core::proto::node_registry::node_registry_service_client::NodeRegistryServiceClient;
use actias_worker_core::proto::node_registry::{HeartbeatRequest, RegisterNodeRequest};
use tonic::transport::Channel;

/// Runs forever; spawn it and forget it.
pub async fn register_and_heartbeat(
    mut client: NodeRegistryServiceClient<Channel>,
    address: String,
    in_flight: Arc<AtomicU32>,
    identity: Arc<std::sync::RwLock<Option<String>>>,
) {
    loop {
        // Registration retries until it lands; the worker serves requests
        // regardless, membership is not on the request path.
        let registration = loop {
            match client
                .register(RegisterNodeRequest {
                    address: address.clone(),
                    capabilities: vec!["http".to_owned()],
                })
                .await
            {
                Ok(response) => break response.into_inner(),
                Err(status) => {
                    warn!(error = %status, "node registration failed, retrying");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        };

        let node_id = registration.node_id;
        let interval = Duration::from_secs(registration.heartbeat_interval_secs.max(1).into());
        info!(node_id, "registered with the placement store");
        // Object claims need to speak as this node; publish the identity.
        *identity.write().expect("no poisoned lock") = Some(node_id.clone());

        loop {
            tokio::time::sleep(interval).await;

            let beat = client
                .heartbeat(HeartbeatRequest {
                    node_id: node_id.clone(),
                    load: in_flight.load(Ordering::Relaxed),
                })
                .await;

            match beat {
                Ok(_) => {}
                Err(status) if status.code() == tonic::Code::NotFound => {
                    warn!(node_id, "node aged out of the registry, re-registering");
                    break;
                }
                // Transient failures keep beating; the ttl absorbs several
                // missed beats before the node ages out.
                Err(status) => warn!(error = %status, "heartbeat failed"),
            }
        }
    }
}
