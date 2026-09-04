//! The http surface: address a script by path or subdomain, resolve it
//! through the pointer and revision caches, run it, and shape what it
//! returns. Everything about placement and object routing lives in
//! [`crate::routing`]; this module only turns transports into calls.
//!
//! [`AppState`] is the one handle every handler reaches through, so a
//! new capability is a field here rather than a second global.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use actias_common::tracing::error;
use axum::Router;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{StatusCode, Uri};
use axum::response::Response;
use core::result::Result::Ok;
use mlua::LuaSerdeExt;
use tonic::transport::Channel;

use actias_common::logging::{live_log_channel, script_log_channel};
use actias_worker_core::egress::EgressClient;
use actias_worker_core::egress::ScopeEgress;
use actias_worker_core::extensions;
use actias_worker_core::extensions::http::Request as LuaRequest;
use actias_worker_core::extensions::log::LogPublisher;
use actias_worker_core::extensions::objects::{DirectoryLister, ObjectRouter};
use actias_worker_core::extensions::sockets::Dialer;
use actias_worker_core::identity::ObjectKey;
use actias_worker_core::objects::ObjectHost;
use actias_worker_core::proto::bundle::File;
use actias_worker_core::proto::kv_service::kv_service_client::KvServiceClient;
use actias_worker_core::proto::node_registry::node_registry_service_client::NodeRegistryServiceClient;
use actias_worker_core::proto::script_service::FindScriptRequest;
use actias_worker_core::proto::script_service::GetAliasRequest;
use actias_worker_core::proto::script_service::LiveScriptSession;
use actias_worker_core::proto::script_service::Revision;
use actias_worker_core::proto::script_service::Script;
use actias_worker_core::proto::script_service::find_script_request::Query;
use actias_worker_core::proto::script_service::script_service_client::ScriptServiceClient;
use actias_worker_core::proto::script_service::{ProjectPolicy, ProjectRef, Region};
use actias_worker_core::runtime::{ActiasRuntime, PreparedRevision};
use actias_worker_core::shares::{RateLimits, Refused, ScopeShares};

use crate::blob_cache::BlobCache;
use crate::metrics::Metrics;
use crate::objects::store::ObjectStore;
use crate::routing::{ObjectRouting, ResolveError, cached_revision};

/// The service clients every request handler needs.
#[derive(Clone)]
pub struct Clients {
    pub script: ScriptServiceClient<actias_worker_core::Grpc>,
    pub kv: KvServiceClient<actias_worker_core::Grpc>,
}

/// Hot-path caches shared across requests.
///
/// The pointer cache maps a public identifier to its script row and expires
/// quickly, so a publish propagates within the ttl. The revision cache holds
/// prepared revisions and is bounded by bytes rather than time, because a
/// revision is immutable; only eviction pressure should drop one.
#[derive(Clone)]
pub struct WorkerCaches {
    pub(crate) pointers: moka::future::Cache<String, Script>,
    pub(crate) revisions: moka::future::Cache<String, Arc<PreparedRevision>>,
    /// Alias pointers (`script_id/name` to revision id); mutable like the
    /// script pointer, so it expires on the same ttl.
    pub(crate) aliases: moka::future::Cache<String, String>,
    /// Object key to the code it runs: the owner script's current
    /// revision. Mutable twice over (the owner can change on publish, the
    /// owner republishes), so it expires on the pointer ttl.
    pub(crate) owners: moka::future::Cache<String, Arc<PreparedRevision>>,
    /// A project's runtime policy (rates, egress lists, the home region),
    /// mutable in the console, so it expires on the pointer ttl.
    pub(crate) policies: moka::future::Cache<String, Arc<ProjectPolicy>>,
    /// Object id to the region the platform moved it to, learned from
    /// the birth region's answer (its own claim here, a hop's reply
    /// elsewhere); a move invalidates it by answering differently.
    pub(crate) moves: moka::future::Cache<String, String>,
    /// The regions the control plane knows, under one key; empty on a
    /// single-region deployment, which forwards nothing.
    pub(crate) regions: moka::future::Cache<String, Arc<Vec<Region>>>,
}

impl WorkerCaches {
    pub fn new(pointer_ttl: Duration, revision_cache_bytes: u64) -> Self {
        Self {
            pointers: moka::future::Cache::builder()
                .max_capacity(10_000)
                .time_to_live(pointer_ttl)
                .build(),
            aliases: moka::future::Cache::builder()
                .max_capacity(10_000)
                .time_to_live(pointer_ttl)
                .build(),
            owners: moka::future::Cache::builder()
                .max_capacity(10_000)
                .time_to_live(pointer_ttl)
                .build(),
            policies: moka::future::Cache::builder()
                .max_capacity(10_000)
                .time_to_live(pointer_ttl)
                .build(),
            moves: moka::future::Cache::builder()
                .max_capacity(100_000)
                .time_to_live(pointer_ttl)
                .build(),
            regions: moka::future::Cache::builder()
                .max_capacity(1)
                .time_to_live(pointer_ttl)
                .build(),
            revisions: moka::future::Cache::builder()
                .max_capacity(revision_cache_bytes)
                .weigher(|_, prepared: &Arc<PreparedRevision>| {
                    prepared.weight().clamp(1, u32::MAX as u64) as u32
                })
                .build(),
        }
    }
}

/// Builds the worker's http surface: every path and method funnels into the
/// script handler, bodies are capped, and the whole request carries a
/// deadline.
pub fn router(state: AppState, max_body_bytes: usize) -> Router {
    Router::new()
        .route("/_metrics", axum::routing::get(metrics_handler))
        .fallback(handle)
        .layer(DefaultBodyLimit::max(max_body_bytes))
        .with_state(state)
}

/// Everything a request handler reaches for.
#[derive(Clone)]
pub struct AppState {
    pub clients: Clients,
    pub caches: WorkerCaches,
    /// Bundle bytes by hash; published revisions hydrate through it.
    pub blobs: BlobCache,
    pub egress: EgressClient,
    /// Carries script log lines out; without it they stay in worker tracing.
    pub redis: Option<redis::aio::ConnectionManager>,
    /// Decrypts stored secrets; without it `secret` declarations error.
    pub secret_client: Option<
        actias_worker_core::proto::secret_service::secret_service_client::SecretServiceClient<
            actias_worker_core::Grpc,
        >,
    >,
    pub request_timeout: Duration,
    /// Work and wall ceilings every guest scope on this node is armed
    /// with: requests, object calls, connection frames alike. One pair,
    /// because a caller cannot tell which kind of scope it reached.
    pub guest_limits: GuestLimits,
    /// Requests currently executing; the heartbeat reports it as load.
    pub in_flight: Arc<AtomicU32>,
    /// Live durable objects on this node, one pinned vm each.
    pub objects: Arc<ObjectHost>,
    /// One SQLite file per object identity lives here.
    pub object_data_dir: std::path::PathBuf,
    /// Size cap per object database, bytes.
    pub object_db_max_bytes: u64,
    /// When objects ship WAL segments and when generations rotate.
    pub ship_thresholds: crate::objects::store::ShipThresholds,
    /// How long a written call's answer waits on the output gate before
    /// its outcome is reported unknown.
    pub ack_gate: std::time::Duration,
    /// Shipping and output-gate counters, shared by every object's
    /// shipper on this node.
    pub ship_gauges: Arc<crate::objects::shipper::ShipGauges>,
    /// How much of the store this node may occupy at once.
    pub ship_limits: Arc<crate::objects::shipper::ShipLimits>,
    /// Settled directory rows waiting to leave this node. Node-wide,
    /// because a delta is a bag of rows: one upload per class per
    /// interval rather than one per object.
    pub directory_sync: Arc<crate::directory::sync::DirectorySyncer>,
    /// The directory's loop counters, served at /_metrics.
    pub directory_gauges: Arc<crate::directory::gauges::DirectoryGauges>,
    /// Live membership as the reader placement last read it, keyed by
    /// one constant; a short ttl bounds how long a query can hop to a
    /// node that left.
    pub reader_membership: moka::future::Cache<String, Arc<Vec<String>>>,
    /// Milliseconds a `directory` function may run before its budget is
    /// spent; contained like any other failure.
    pub directory_eval_budget_ms: u64,
    /// Class overlays this node has materialized for querying. Keyed by
    /// generation, and bases are immutable, so rebuilding is the only
    /// invalidation.
    pub directory_overlays: Arc<crate::directory::read::Overlays>,
    /// Rows recomputed by a verified read's scratch tail, keyed by the
    /// version they were derived at. A failed derivation is a class-wide
    /// condition, not a scattered one, so without this every visit over
    /// a broken class re-restores every object it names.
    pub directory_recomputed: Arc<crate::directory::visit::Recomputed>,
    /// Names an admission gate refused, with the refusal; junk
    /// identities answer from here for a pointer ttl.
    pub admit_refusals: moka::future::Cache<String, String>,
    /// Per-script request counters, served at /_metrics.
    pub metrics: Arc<Metrics>,
    /// Where object snapshots ship to and restore from.
    pub object_store: Arc<ObjectStore>,
    /// How long a snapshot replica serves reads before refreshing.
    pub replica_ttl: Duration,
    /// Bumped each time this node registers with the placement store.
    /// A residency's shipper re-reads the store's fence when it moves,
    /// because a re-registration is the one event under which another
    /// node can have taken a lease this node believed it held.
    pub membership_generation: Arc<std::sync::atomic::AtomicU64>,
    /// What this node holds for other owners' objects, and the counters
    /// of both sides of tail replication.
    pub replica_store: Arc<crate::objects::replica::ReplicaStore>,
    /// Replica nodes an owner on this node fans out to.
    pub replica_count: usize,
    /// Acks that answer a written call; 0 is shadow mode.
    pub replica_quorum: usize,
    /// Longest a fan-out waits for one replica.
    pub replica_ack: Duration,
    /// Warm vms per revision, taken by requests, connections and objects.
    pub vm_pool: Arc<crate::vm_pool::VmPool>,
    /// This node's data-plane address as registered, so a stale
    /// incarnation of this very node is never chosen as its replica.
    pub node_address: String,
    /// The region this node runs in; an object whose home is another
    /// region is forwarded there, and never spawned here.
    pub region: String,
    /// This node's bounds, split fairly among the scopes using them.
    pub shares: Arc<ScopeShares>,
    /// Per-project request and work rates, from each project's policy.
    pub rates: Arc<RateLimits>,
    /// Every resident object's shipping state, for the watermark a
    /// replica asks before serving a read.
    pub ship_states: Arc<
        std::sync::Mutex<std::collections::HashMap<String, Arc<crate::objects::store::Watermark>>>,
    >,
    /// Channels to peer workers' data planes, by address; lazy, so a dead
    /// peer costs its caller the failure, never a held-up cache.
    pub peers: moka::future::Cache<String, Channel>,
    /// Where a non-resident object lives, learned from refused claims;
    /// a stale entry costs one wrong-home hop, which invalidates it.
    /// This is what keeps the placement store off the call hot path.
    pub holders: moka::future::Cache<String, String>,
    /// A node id's data-plane address; ids never change address, so the
    /// ttl only bounds how long a dead id lingers.
    pub node_addrs: moka::future::Cache<String, String>,
    /// The cluster-internal secret every data-plane call carries.
    pub internal_token: String,
    /// Revisions whose cron events were already armed by this process;
    /// arming is idempotent, this only spares the calls.
    pub armed_crons: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    /// Idle time before a pinned vm hibernates.
    pub object_idle_after: Duration,
    /// Queue delivery limits every pinned task spawns with.
    pub queue_policy: actias_worker_core::platform::queue::QueuePolicy,
    /// This node's registry identity, filled in once registration lands;
    /// object claims speak as it.
    pub node_identity: Arc<std::sync::RwLock<Option<String>>>,
    /// Every live snapshot shipper, for the drain flush.
    pub shippers: crate::objects::shipper::Shippers,
    /// The placement store, for object lease claims.
    pub registry: NodeRegistryServiceClient<actias_worker_core::Grpc>,
    /// Domain subdomain routing hangs off; [`None`] leaves only the path
    /// forms.
    pub base_domain: Option<String>,
    /// Live websocket connections on this node, by connection id; the
    /// stream pump delivers connection edges through it, and a missing
    /// id is the prune signal.
    pub connections: Arc<actias_worker_core::connections::ConnectionRegistry>,
    /// Warm/hibernated counts and wake costs, updated by every
    /// connection actor and rendered at scrape time.
    pub connection_gauges: Arc<actias_worker_core::connections::actor::ConnectionGauges>,
    /// Silence before a connection's vm drops; [`None`] never drops.
    pub connection_hibernate_after: Option<std::time::Duration>,
}

/// What a guest scope may spend before the platform cuts it off.
/// Operator config, defaulted from
/// [`actias_worker_core::budget::DEFAULT_WORK_LIMIT`].
#[derive(Clone, Copy)]
pub struct GuestLimits {
    /// Work units; the ceiling that actually stops a runaway.
    pub work: u64,
    /// Wall seconds, the backstop for code stuck outside the vm.
    pub wall_secs: u64,
}

impl GuestLimits {
    /// Arms a freshly built vm. Every runtime this node constructs goes
    /// through here, so the wall limit a constructor took and the work
    /// limit a setter carries cannot drift apart.
    pub fn apply(&self, runtime: &ActiasRuntime) {
        runtime.set_work_limit(self.work);
    }
}

/// Holds the in-flight gauge up for exactly one request's lifetime;
/// dropping on any exit path, including panics and timeouts, keeps the
/// gauge honest.
struct InFlight(Arc<AtomicU32>);

impl InFlight {
    fn enter(gauge: &Arc<AtomicU32>) -> Self {
        gauge.fetch_add(1, Ordering::Relaxed);
        Self(gauge.clone())
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// What a request addresses, by path or by subdomain.
#[derive(Debug, PartialEq)]
enum Route<'a> {
    /// The current published revision of a script.
    Published { identifier: &'a str },
    /// One live development session's working tree.
    Live {
        identifier: &'a str,
        session: &'a str,
    },
    /// One specific revision, current or not: a preview url.
    Revision {
        identifier: &'a str,
        revision: &'a str,
    },
    /// A named environment: the revision its alias points at.
    Aliased { identifier: &'a str, alias: &'a str },
}

impl Route<'_> {
    /// Path segments the path form consumed, which the script never sees.
    /// Host-routed requests consume none.
    fn consumed_segments(&self) -> usize {
        match self {
            Route::Published { .. } => 1,
            Route::Live { .. } | Route::Revision { .. } | Route::Aliased { .. } => 3,
        }
    }
}

/// Extracts the target a request path addresses.
///
/// The first segment selects the script, so `/my-script/users` runs the
/// published `my-script` and hands it `/users`. The `_live` and `_rev`
/// prefixes are reserved: `/_live/my-script/<session>/users` runs that
/// script's live session and `/_rev/my-script/<revision>/users` previews
/// one revision, so no published script named `_live` or `_rev` is
/// reachable. A path missing any needed segment, such as `/`, addresses
/// nothing.
fn route_by_path(path: &str) -> Option<Route<'_>> {
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());

    match segments.next()? {
        "_live" => Some(Route::Live {
            identifier: segments.next()?,
            session: segments.next()?,
        }),
        "_rev" => Some(Route::Revision {
            identifier: segments.next()?,
            revision: segments.next()?,
        }),
        "_alias" => Some(Route::Aliased {
            identifier: segments.next()?,
            alias: segments.next()?,
        }),
        identifier => Some(Route::Published { identifier }),
    }
}

/// Extracts the target a Host header addresses under the base domain.
///
/// `my-script.<base>` serves the published script,
/// `my-script--live-<session>.<base>` one live session and
/// `my-script--r-<revision>.<base>` a revision preview. `--` never occurs
/// in a public identifier, so the marker split is unambiguous. A host
/// outside the base domain, or nested deeper than one label, addresses
/// nothing and falls back to path routing.
fn route_by_host<'a>(host: &'a str, base: &str) -> Option<Route<'a>> {
    // The Host header may carry a port; the base never does.
    let host = host.split(':').next()?;
    let label = host.strip_suffix(base)?.strip_suffix('.')?;

    if label.is_empty() || label.contains('.') {
        return None;
    }

    if let Some((identifier, session)) = label.split_once("--live-") {
        return Some(Route::Live {
            identifier,
            session,
        });
    }

    if let Some((identifier, revision)) = label.split_once("--r-") {
        return Some(Route::Revision {
            identifier,
            revision,
        });
    }

    // Any other `--` names an environment alias; `live-` and `r-` prefixes
    // are refused at alias creation, so the markers above cannot collide.
    if let Some((identifier, alias)) = label.split_once("--") {
        return Some(Route::Aliased { identifier, alias });
    }

    Some(Route::Published { identifier: label })
}

/// How the addressed script is served, owned past the routing borrow.
enum Target {
    Published,
    Live(String),
    Preview(String),
    Aliased(String),
}

/// A preview naming a revision that belongs to a different script; the
/// loader refuses it before anything is cached.
pub(crate) const FOREIGN_REVISION: &str = "the revision does not belong to this script";

/// Whether a load failed because the addressed thing does not exist for
/// this script, as opposed to infrastructure failing.
fn target_absent(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<tonic::Status>()
            .is_some_and(|status| status.code() == tonic::Code::NotFound)
            || cause.to_string() == FOREIGN_REVISION
    })
}

/// Builds a response whose body is exactly `body`.
fn text_response(status: StatusCode, body: &'static str) -> Response {
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    response
}

/// The answer to a project over its share of this node: refused at
/// once rather than queued, with when to try again, so a saturating
/// tenant costs no vm and no waiting slot.
fn refused_response(refused: &Refused) -> Response {
    let mut response = text_response(
        StatusCode::TOO_MANY_REQUESTS,
        "This project is over its share of this node; retry shortly.",
    );
    if let Ok(value) = axum::http::HeaderValue::from_str(&refused.retry_after.as_secs().to_string())
    {
        response
            .headers_mut()
            .insert(axum::http::header::RETRY_AFTER, value);
    }
    response
}

/// Builds the response for a request the runtime could not complete.
///
/// The cause is logged against a correlation id and the client is told only the
/// id, because internal errors quote connection strings, hostnames and paths.
fn internal_error_response(error: &anyhow::Error) -> Response {
    let correlation_id = uuid::Uuid::new_v4();

    error!(
        error = %error,
        correlation_id = %correlation_id,
        "error handling request"
    );

    let mut response = Response::new(Body::from(format!(
        "Internal error. Correlation ID: {correlation_id}"
    )));
    *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
    response
}

/// Unwraps the shared error a moka loader hands back into an owned one.
///
/// `try_get_with` clones one load error to every caller that piled onto the
/// same miss, so it arrives in an [`Arc`] and only its rendering survives.
fn cache_load_error(error: Arc<anyhow::Error>) -> anyhow::Error {
    anyhow::anyhow!("{error:#}")
}

/// Resolves a public identifier to its script row through the pointer cache.
///
/// Cache misses resolve through the loader; moka deduplicates concurrent
/// misses of one key into a single backend call. Failed loads are not
/// cached, so an unknown identifier costs a lookup every time. The error
/// keeps its cause chain, so the caller can tell an absent script from
/// infrastructure failing.
/// The project's runtime policy, cached on the pointer ttl. The platform
/// defaults when the control plane does not answer: policy narrows what
/// a project may do, and is never a gate on serving it.
/// The data-plane address of `region`, or [`None`] when the control
/// plane does not know it: an unknown region is served where the call
/// landed rather than failed (FLEET.md P4.b). The list is one cached
/// read per pointer ttl; a fetch that fails reads as no regions.
pub(crate) async fn region_address(state: &AppState, region: &str) -> Option<String> {
    let client = state.clients.script.clone();
    let regions = state
        .caches
        .regions
        .get_with("all".to_owned(), async move {
            let mut client = client;
            match client.list_regions(()).await {
                Ok(listed) => Arc::new(listed.into_inner().regions),
                Err(error) => {
                    actias_common::tracing::warn!(%error, "the regions could not be listed; forwarding nothing");
                    Arc::new(Vec::new())
                }
            }
        })
        .await;
    regions
        .iter()
        .find(|known| known.name == region)
        .map(|known| known.data_plane_addr.clone())
}

pub(crate) async fn project_policy(state: &AppState, project_id: &str) -> Arc<ProjectPolicy> {
    let client = state.clients.script.clone();
    let key = project_id.to_owned();
    state
        .caches
        .policies
        .get_with(project_id.to_owned(), async move {
            let mut client = client;
            match client
                .get_project_policy(ProjectRef {
                    project_id: key.clone(),
                })
                .await
            {
                Ok(policy) => Arc::new(policy.into_inner()),
                Err(status) => {
                    actias_common::tracing::debug!(
                        project_id = key,
                        error = %status,
                        "project policy unreadable; running on the defaults"
                    );
                    Arc::new(ProjectPolicy {
                        project_id: key,
                        ..Default::default()
                    })
                }
            }
        })
        .await
}

/// The policy's host lists, as the egress check wants them.
pub(crate) fn scope_egress(policy: &ProjectPolicy) -> ScopeEgress {
    ScopeEgress::new(policy.egress_allow.clone(), policy.egress_deny.clone())
}

async fn resolve_script(
    caches: &WorkerCaches,
    client: &ScriptServiceClient<actias_worker_core::Grpc>,
    identifier: String,
) -> Result<Script, Arc<anyhow::Error>> {
    caches
        .pointers
        .try_get_with(identifier.clone(), {
            let mut client = client.clone();
            async move {
                let script = client
                    .query_script(FindScriptRequest {
                        query: Some(Query::PublicName(identifier)),
                    })
                    .await?;
                Ok::<_, anyhow::Error>(script.into_inner())
            }
        })
        .await
}

/// Path the script or asset lookup sees: the request path with the routing
/// segments the route consumed removed. A trailing slash survives because it
/// selects a directory's index asset.
fn script_relative_path(path: &str, consumed_segments: usize) -> String {
    let relative = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .skip(consumed_segments)
        .collect::<Vec<_>>()
        .join("/");

    if !relative.is_empty() && path.ends_with('/') {
        format!("{relative}/")
    } else {
        relative
    }
}

/// Serves one bundle asset: content type from the manifest, the blake3 hash
/// as a strong etag, and a 304 when the client already holds these bytes.
fn asset_response(
    file: &File,
    if_none_match: Option<&axum::http::HeaderValue>,
) -> anyhow::Result<Response> {
    use axum::http::header;

    // Live sessions may carry hashless files; those simply serve uncached.
    let etag = (!file.hash.is_empty()).then(|| format!("\"{}\"", file.hash));

    let revalidated = match (&etag, if_none_match) {
        (Some(etag), Some(held)) => held
            .to_str()
            .map(|held| {
                held.split(',')
                    .map(|candidate| candidate.trim().trim_start_matches("W/"))
                    .any(|candidate| candidate == etag || candidate == "*")
            })
            .unwrap_or(false),
        _ => false,
    };

    let mut response = Response::new(if revalidated {
        Body::empty()
    } else {
        Body::from(file.content.clone())
    });
    if revalidated {
        *response.status_mut() = StatusCode::NOT_MODIFIED;
    }

    let headers = response.headers_mut();
    let content_type = if file.content_type.is_empty() {
        "application/octet-stream"
    } else {
        file.content_type.as_str()
    };
    headers.insert(header::CONTENT_TYPE, content_type.parse()?);
    if let Some(etag) = etag {
        headers.insert(header::ETAG, etag.parse()?);
    }

    Ok(response)
}

/// Converts a lua response table into the wire response.
fn lua_response_into_response(res: extensions::http::Response) -> anyhow::Result<Response> {
    let mut response = Response::new(match res.body {
        Some(body) => body.into_axum_body(),
        None => Body::empty(),
    });

    *response.status_mut() = StatusCode::from_u16(res.status_code.unwrap_or(200))?;

    if let Some(headers) = res.headers {
        let headers_mut = response.headers_mut();
        for (key, value) in headers {
            headers_mut.insert(
                axum::http::header::HeaderName::from_bytes(key.as_bytes())?,
                value.parse()?,
            );
        }
    }

    Ok(response)
}

/// The prometheus exposition; gauges are measured at scrape time.
async fn metrics_handler(State(state): State<AppState>) -> Response {
    let resident = state.objects.resident_count().await;
    let mut response = Response::new(Body::from(
        state.metrics.render(
            resident,
            &state.connection_gauges,
            &state.ship_gauges,
            &state.replica_store.gauges,
            (&state.vm_pool.gauges, state.vm_pool.warm_count()),
            &state.directory_gauges,
            (
                state
                    .object_store
                    .file_reads
                    .load(std::sync::atomic::Ordering::Relaxed),
                state
                    .object_store
                    .file_fetches
                    .load(std::sync::atomic::Ordering::Relaxed),
            ),
            (
                state
                    .object_store
                    .chunk_puts
                    .load(std::sync::atomic::Ordering::Relaxed),
                state
                    .object_store
                    .chunk_bytes_put
                    .load(std::sync::atomic::Ordering::Relaxed),
                state
                    .object_store
                    .chunk_gets
                    .load(std::sync::atomic::Ordering::Relaxed),
            ),
            &state.shares,
        ),
    ));
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("text/plain; version=0.0.4"),
    );
    response
}

/// The script a request addresses, as a metrics label; parsed the same
/// way run_script routes, but never failing.
fn metrics_label(state: &AppState, request: &axum::extract::Request) -> String {
    let host_route = state.base_domain.as_deref().and_then(|base| {
        request
            .headers()
            .get(axum::http::header::HOST)
            .and_then(|value| value.to_str().ok())
            .and_then(|host| route_by_host(host, base))
    });

    let route = host_route.or_else(|| route_by_path(request.uri().path()));
    match route {
        Some(
            Route::Published { identifier }
            | Route::Live { identifier, .. }
            | Route::Revision { identifier, .. }
            | Route::Aliased { identifier, .. },
        ) => identifier.to_owned(),
        None => "(none)".to_owned(),
    }
}

/// The live session a request addresses, when it addresses one; parsed
/// the same way run_script routes, but never failing.
fn live_session_of(state: &AppState, request: &axum::extract::Request) -> Option<String> {
    let host_route = state.base_domain.as_deref().and_then(|base| {
        request
            .headers()
            .get(axum::http::header::HOST)
            .and_then(|value| value.to_str().ok())
            .and_then(|host| route_by_host(host, base))
    });
    match host_route.or_else(|| route_by_path(request.uri().path())) {
        Some(Route::Live { session, .. }) => Some(session.to_owned()),
        _ => None,
    }
}

/// Handles every inbound request by running the addressed script.
///
/// No manual span here: the TraceExtract layer already wraps every
/// request in one, and a guard held across an await leaks the span onto
/// the executor thread, chaining unrelated tasks into one trace.
async fn handle(State(state): State<AppState>, request: axum::extract::Request) -> Response {
    let _in_flight = InFlight::enter(&state.in_flight);

    let label = metrics_label(&state, &request);
    let live_session = live_session_of(&state, &request);
    let metrics = state.metrics.clone();
    let started = std::time::Instant::now();

    let deadline = state.request_timeout;
    let redis = state.redis.clone();
    let caches = state.caches.clone();
    let script_client = state.clients.script.clone();
    let result = tokio::time::timeout(deadline, run_script(state, request)).await;

    // A failure's audience is whoever is watching: a live session's
    // error joins the session stream, where the workbench and
    // `actias dev` follow; a published request's joins the script's
    // tail, where the dashboard's Logs and `actias tail` follow. Two
    // audiences, two streams, never crossed. The http response stays
    // sanitized either way.
    let report_error = |text: String| {
        let redis = redis.clone();
        let caches = caches.clone();
        let script_client = script_client.clone();
        let identifier = label.clone();
        let session = live_session.clone();
        async move {
            let Some(redis) = redis else { return };
            match session {
                Some(session) => {
                    LogPublisher::new(redis, live_log_channel(&session)).publish("error", text);
                }
                None => {
                    if let Ok(script) = resolve_script(&caches, &script_client, identifier).await {
                        LogPublisher::new(redis, script_log_channel(&script.id))
                            .publish("error", text);
                    }
                }
            }
        }
    };

    let response = match result {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => {
            report_error(format!("{error:#}")).await;
            internal_error_response(&error)
        }
        Err(_elapsed) => {
            report_error("Script did not respond in time.".to_owned()).await;
            text_response(
                StatusCode::GATEWAY_TIMEOUT,
                "Script did not respond in time.",
            )
        }
    };

    // Only an identifier that actually resolved becomes a series: the
    // pointer cache holds it by now if it did, and carries the project
    // the dashboards narrow by. Everything else (favicon probes, path
    // spam) folds into one bucket, or unbounded label cardinality would
    // let arbitrary urls mint prometheus series.
    let (project, label) = match caches.pointers.get(&label).await {
        Some(script) => (script.project_id, label),
        None => ("(unknown)".to_owned(), "(unknown)".to_owned()),
    };
    // An error is the script failing, not the script answering: 4xx and
    // redirects are responses, only 5xx counts against the script.
    metrics.record(
        &project,
        &label,
        started.elapsed(),
        !response.status().is_server_error(),
    );
    response
}

/// Resolves the script, runs it, and shapes its response.
async fn run_script(state: AppState, request: axum::extract::Request) -> anyhow::Result<Response> {
    // DefaultBodyLimit only takes effect through extractors, so the body is
    // wrapped explicitly; without this the cap silently would not apply.
    use axum::RequestExt;
    let request = request.with_limited_body();

    let (mut parts, body) = request.into_parts();

    // A websocket handshake IS web traffic: taken off the request here,
    // and the fetch request table gains `request.upgrade` only when this
    // is Some, so the Lua-side truthy check and this extraction agree.
    let websocket = {
        use axum::extract::FromRequestParts;
        axum::extract::ws::WebSocketUpgrade::from_request_parts(&mut parts, &())
            .await
            .ok()
    };

    // Subdomain routing wins when a base domain is configured and the Host
    // header sits under it; anything else falls back to the path forms, so
    // compose and direct-ip access keep working.
    let host_route = state.base_domain.as_deref().and_then(|base| {
        parts
            .headers
            .get(axum::http::header::HOST)
            .and_then(|value| value.to_str().ok())
            .and_then(|host| route_by_host(host, base))
    });
    let (route, consumed_segments) = match host_route {
        // The whole path belongs to the script when the host routed.
        Some(route) => (route, 0),
        None => match route_by_path(parts.uri.path()) {
            Some(route) => {
                let consumed = route.consumed_segments();
                (route, consumed)
            }
            None => return Ok(text_response(StatusCode::NOT_FOUND, "Invalid script.")),
        },
    };
    let (identifier, target) = match route {
        Route::Published { identifier } => (identifier.to_owned(), Target::Published),
        Route::Live {
            identifier,
            session,
        } => (identifier.to_owned(), Target::Live(session.to_owned())),
        Route::Revision {
            identifier,
            revision,
        } => (identifier.to_owned(), Target::Preview(revision.to_owned())),
        Route::Aliased { identifier, alias } => {
            (identifier.to_owned(), Target::Aliased(alias.to_owned()))
        }
    };

    // The body is read before anything else so the size cap rejects oversized
    // requests without spending gRPC calls on them.
    let body = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return Ok(text_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Request body exceeds the limit.",
            ));
        }
    };

    // An identifier nobody owns is the visitor's typo (or a browser probing
    // for /favicon.ico at the root), not an incident: a plain 404, no
    // correlation id, no error log.
    let script = match resolve_script(&state.caches, &state.clients.script, identifier).await {
        Ok(script) => script,
        Err(error) if target_absent(&error) => {
            return Ok(text_response(
                StatusCode::NOT_FOUND,
                "No script with that identifier.",
            ));
        }
        Err(error) => return Err(cache_load_error(error)),
    };

    // Admission by fair share: a project over its share of this node's
    // requests is answered now, not queued, and the permit lives for
    // the request. A connection the request upgrades into holds its own
    // permit from the connections pool.
    let _admitted = match state.shares.requests.try_acquire(&script.project_id) {
        Ok(permit) => permit,
        Err(refused) => return Ok(refused_response(&refused)),
    };
    // Then by the project's own rates: requests per second now, work
    // units charged after the call so an overspending project pays on
    // its next request rather than mid-call.
    let project_id = script.project_id.clone();
    let policy = project_policy(&state, &project_id).await;
    if let Err(refused) = state.rates.admit(
        &project_id,
        policy.requests_per_sec,
        policy.work_units_per_sec,
    ) {
        return Ok(refused_response(&refused));
    }

    // Live output goes to the session's channel where `actias dev` tails it;
    // published scripts log to a per-script channel for `actias tail`.
    let log_channel = match &target {
        Target::Live(session_id) => live_log_channel(session_id),
        _ => script_log_channel(&script.id),
    };
    let logs = state
        .redis
        .clone()
        .map(|connection| LogPublisher::new(connection, log_channel));

    let prepared = match target {
        // A live session is the developer's working tree, updated on every
        // save; serving it stale defeats its purpose, so nothing about it is
        // cached and every request fetches the session's current bundle.
        Target::Live(session_id) => {
            let mut client = state.clients.script.clone();

            let live = match client
                .get_live_session(LiveScriptSession {
                    script_id: script.id.clone(),
                    session_id,
                })
                .await
            {
                Ok(live) => live.into_inner(),
                Err(status) if status.code() == tonic::Code::NotFound => {
                    return Ok(text_response(
                        StatusCode::NOT_FOUND,
                        "No live session with that id.",
                    ));
                }
                Err(status) => return Err(status.into()),
            };

            Arc::new(PreparedRevision::prepare(
                script,
                Revision {
                    bundle: live.bundle,
                    ..Default::default()
                },
            )?)
        }
        Target::Published => {
            let Some(revision_id) = script.current_revision_id.clone() else {
                return Ok(text_response(
                    StatusCode::NOT_FOUND,
                    "Script did not have a revision.",
                ));
            };

            cached_revision(&state, script, revision_id)
                .await
                .map_err(cache_load_error)?
        }
        // A preview serves one revision, current or not, through the same
        // immutable cache; only its failure shape differs, because the
        // revision id came from the url rather than the script row.
        Target::Preview(revision_id) => match cached_revision(&state, script, revision_id).await {
            Ok(prepared) => prepared,
            Err(error) if target_absent(&error) => {
                return Ok(text_response(
                    StatusCode::NOT_FOUND,
                    "No such revision for this script.",
                ));
            }
            Err(error) => return Err(cache_load_error(error)),
        },
        // An alias is one pointer lookup away from the preview path: the
        // pointer expires like the script pointer, the revision it names
        // rides the immutable cache.
        Target::Aliased(alias) => {
            let looked_up = state
                .caches
                .aliases
                .try_get_with(format!("{}/{}", script.id, alias), {
                    let mut client = state.clients.script.clone();
                    let script_id = script.id.clone();
                    async move {
                        let alias = client
                            .get_alias(GetAliasRequest {
                                script_id,
                                name: alias,
                            })
                            .await?;
                        Ok::<_, anyhow::Error>(alias.into_inner().revision_id)
                    }
                })
                .await;

            let revision_id = match looked_up {
                Ok(revision_id) => revision_id,
                Err(error) if target_absent(&error) => {
                    return Ok(text_response(
                        StatusCode::NOT_FOUND,
                        "No such alias for this script.",
                    ));
                }
                Err(error) => return Err(cache_load_error(error)),
            };

            cached_revision(&state, script, revision_id)
                .await
                .map_err(cache_load_error)?
        }
    };

    let relative_path = script_relative_path(parts.uri.path(), consumed_segments);

    // A path-routed script root without its trailing slash breaks every
    // relative url the page carries ("app.js" resolving beside the
    // script instead of inside it), so the canonical form is enforced
    // the way webservers always have: redirect, let the browser
    // re-anchor. Host-routed requests consume no segments and already
    // serve the script at "/".
    if consumed_segments > 0 && relative_path.is_empty() && !parts.uri.path().ends_with('/') {
        let location = match parts.uri.query() {
            Some(query) => format!("{}/?{query}", parts.uri.path()),
            None => format!("{}/", parts.uri.path()),
        };
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::PERMANENT_REDIRECT;
        response.headers_mut().insert(
            axum::http::header::LOCATION,
            axum::http::HeaderValue::from_str(&location)?,
        );
        return Ok(response);
    }

    // A GET or HEAD naming an asset is answered from the bundle itself: no
    // vm is created and the script never observes the request. Any other
    // method falls through to the script.
    if matches!(
        parts.method,
        axum::http::Method::GET | axum::http::Method::HEAD
    ) && let Some(file) = prepared.asset(&relative_path)
    {
        return asset_response(file, parts.headers.get(axum::http::header::IF_NONE_MATCH));
    }

    // Create a context URI without the routing segments, used for better
    // routing inside the script.
    let old_uri = &parts.uri;
    let mut context_uri = Uri::builder().path_and_query(format!(
        "/{}{}",
        relative_path,
        match old_uri.query() {
            Some(v) => format!("?{}", v),
            None => "".to_string(),
        }
    ));

    if let Some(scheme) = old_uri.scheme() {
        context_uri = context_uri.scheme(scheme.clone());
    }

    if let Some(auth) = old_uri.authority() {
        context_uri = context_uri.authority(auth.clone());
    }

    let lua_request = LuaRequest::from_parts(
        parts.method.to_string(),
        parts.uri.to_string(),
        Some(context_uri.build()?.to_string()),
        parts
            .headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect(),
        format!("{:?}", parts.version),
        body.to_vec(),
    );

    // Lua futures are Send under mlua's send feature, so the runtime runs
    // directly on the async executor. Never wrap it in block_in_place or
    // a LocalSet: that blocks a worker thread for the whole script.

    // Kept rather than discarded: the call seam and the listing seam
    // are both cut from one routing, so they cannot disagree about
    // whose project a vm is running in.
    // The request's output gate: local object writes answer at commit,
    // and every gate they defer is settled once, before the response or
    // any outbound request leaves. A chain of writes waits one flight,
    // not one per hop.
    let gates = Arc::new(actias_worker_core::objects::PendingGates::default());
    let request_routing = ObjectRouting::deferring(&state, prepared.clone(), gates.clone());
    let router = request_routing.as_router();

    // First touch of a revision on this worker arms its cron events: each
    // becomes a __cron object whose alarm re-arms itself forever after,
    // through hibernation and restarts via the sweep.
    let cron_events = prepared.cron_events();
    if !cron_events.is_empty() && !prepared.revision_id.is_empty() {
        let fresh = state
            .armed_crons
            .lock()
            .expect("no poisoned lock")
            .insert(prepared.revision_id.clone());
        if fresh {
            let routing = ObjectRouting::new(&state, prepared.clone());
            let armed_crons = state.armed_crons.clone();
            let revision_id = prepared.revision_id.clone();
            tokio::spawn(async move {
                let mut all_armed = true;
                for event in cron_events {
                    let key = ObjectKey::scoped(
                        &routing.prepared.script.project_id,
                        &routing.prepared.script.id,
                        actias_worker_core::extensions::objects::CRON_CLASS,
                        &event,
                    );
                    let armed = match routing.resolve_handle(&key, false).await {
                        Err(ResolveError::Elsewhere(holder)) => {
                            Err(format!("homed on {holder}; its node arms it"))
                        }
                        Err(ResolveError::Moved(region)) => {
                            Err(format!("lives in region {region}; its region arms it"))
                        }
                        Err(ResolveError::Other(error)) => Err(error),
                        Ok(handle) => handle
                            .call(
                                "__dispatch",
                                serde_json::json!({
                                    "class": actias_worker_core::extensions::objects::CRON_CLASS,
                                    "name": event,
                                    "method": "ensure",
                                    "args": [event],
                                    // The object's own key: set_alarm persists
                                    // it, and the sweep needs it to revive this
                                    // cron after a restart.
                                    "chain": [key.to_string()],
                                }),
                            )
                            .await
                            .map(|_| ())
                            .map_err(|e| e.to_string()),
                    };
                    if let Err(error) = armed {
                        // Not armed here does not mean armed nowhere; but a
                        // skip must not be permanent, so the next request
                        // retries (the incumbent may die).
                        all_armed = false;
                        error!(%error, event, "cron event could not be armed");
                    }
                }
                if !all_armed {
                    armed_crons
                        .lock()
                        .expect("no poisoned lock")
                        .remove(&revision_id);
                }
            });
        }
    }

    let lua = state
        .vm_pool
        .take(
            crate::vm_pool::VmKey {
                revision_id: prepared.revision_id.clone(),
                flavor: crate::vm_pool::Flavor::Request,
            },
            request_vm_build(state.clone(), prepared.clone(), logs.clone()),
        )
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    state.guest_limits.apply(&lua);
    // A connection outlives the request and settles no gates: its edge
    // writes are held until durable like any object's.
    let connection_router = ObjectRouting::new(&state, prepared.clone()).as_router();
    lua.set_app_data::<ObjectRouter>(router);
    // A request may open a wire outward; the connection outlives it.
    lua.set_app_data::<Dialer>(dialer_for(state.clone(), prepared.clone(), logs.clone()));
    // The listing seam. Without it every `Class:list` in a request
    // handler refuses, because the verb resolves against app data that
    // only this call installs.
    lua.set_app_data::<DirectoryLister>(request_routing.as_lister());
    lua.set_app_data(scope_egress(&policy));
    lua.set_app_data::<Arc<actias_worker_core::objects::PendingGates>>(gates.clone());

    let listener = lua.listener(ActiasRuntime::FETCH_EVENT)?;

    let request_value = lua.to_value(&lua_request)?;
    if websocket.is_some()
        && let mlua::Value::Table(request_table) = &request_value
    {
        actias_worker_core::extensions::sockets::arm_request(&lua, request_table)?;
    }

    lua.start_timer();

    let value: mlua::Value = listener.call_async(request_value).await?;

    // Nothing leaves before the writes it may describe are durable.
    gates
        .settle()
        .await
        .map_err(|error| anyhow::anyhow!("a write's outcome is unknown: {error}"))?;

    // The handler upgraded: the response is the handshake and the
    // request vm is released like any other response. The pending
    // carries a class name and a json seed, and the actor rebuilds a
    // vm of this same revision from the factory when a handler needs
    // one.
    state
        .rates
        .charge(&project_id, lua.consumed().work, policy.work_units_per_sec);

    if let Some(pending) =
        lua.remove_app_data::<actias_worker_core::extensions::sockets::PendingUpgrade>()
    {
        let websocket = websocket.ok_or_else(|| {
            // arm_request only runs when the handshake exists, so this
            // is a platform bug, not a script mistake.
            anyhow::anyhow!("an upgrade was parked without a websocket handshake")
        })?;
        let registry = state.connections.clone();
        // The connection remembers which node hosts it, so a publisher
        // homed elsewhere knows where its events must travel.
        let node = state
            .node_identity
            .read()
            .ok()
            .and_then(|guard| guard.clone())
            .unwrap_or_default();
        let about = actias_worker_core::extensions::sockets::About {
            connection_class: pending.spec.name.clone(),
            direction: Some(actias_worker_core::connections::Direction::Inbound),
            peer: None,
            project_id: prepared.script.project_id.clone(),
            script_id: prepared.script.id.clone(),
            opened_at_ms: actias_worker_core::extensions::objects::unix_now_ms(),
        };
        // The connection's share is taken before the handshake answers,
        // so a project at its bound is refused with a status, never
        // with a wire that closes at once.
        let permit = match state
            .shares
            .connections
            .try_acquire(&prepared.script.project_id)
        {
            Ok(permit) => Some(permit),
            Err(_) => {
                return Ok(text_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    TOO_MANY_CONNECTIONS,
                ));
            }
        };
        let factory = vm_factory(state.clone(), prepared, logs);
        let hibernate_after = state.connection_hibernate_after;
        let gauges = state.connection_gauges.clone();
        let spawn = ConnectionSpawn {
            factory,
            pending,
            registry,
            router: connection_router,
            node,
            hibernate_after,
            gauges,
            about,
            permit,
        };
        return Ok(
            websocket.on_upgrade(move |socket| drive_wire(Wire::Inbound(Box::new(socket)), spawn))
        );
    }

    let lua_response: extensions::http::Response = lua.from_value(value)?;

    lua_response_into_response(lua_response)
}

/// The request-flavoured construction: a vm of one revision with the
/// wall backstop armed, before the request's own pieces are attached.
/// The pool refills through the same closure.
fn request_vm_build(
    state: AppState,
    prepared: Arc<PreparedRevision>,
    logs: Option<LogPublisher>,
) -> crate::vm_pool::VmBuild {
    Arc::new(move || {
        let state = state.clone();
        let prepared = prepared.clone();
        let logs = logs.clone();
        Box::pin(async move {
            ActiasRuntime::new(
                prepared,
                state.clients.kv.clone(),
                state.egress.clone(),
                logs,
                state.secret_client.clone(),
                Some(state.guest_limits.wall_secs),
            )
            .await
            .map_err(|error| error.to_string())
        })
    })
}

/// Builds vms of one revision for a connection's actor: the same
/// construction the request path uses, minus the request.
fn vm_factory(
    state: AppState,
    prepared: Arc<PreparedRevision>,
    logs: Option<LogPublisher>,
) -> actias_worker_core::connections::actor::VmFactory {
    Arc::new(move || {
        let state = state.clone();
        let prepared = prepared.clone();
        let logs = logs.clone();
        Box::pin(async move {
            let lua = state
                .vm_pool
                .take(
                    crate::vm_pool::VmKey {
                        revision_id: prepared.revision_id.clone(),
                        flavor: crate::vm_pool::Flavor::Request,
                    },
                    request_vm_build(state.clone(), prepared.clone(), logs.clone()),
                )
                .await?;
            state.guest_limits.apply(&lua);
            let routing = ObjectRouting::new(&state, prepared.clone());
            lua.set_app_data::<ObjectRouter>(routing.as_router());
            lua.set_app_data::<DirectoryLister>(routing.as_lister());
            let policy = project_policy(&state, &prepared.script.project_id).await;
            lua.set_app_data(scope_egress(&policy));
            lua.set_app_data::<Dialer>(dialer_for(state.clone(), prepared, logs));
            Ok(lua)
        })
    })
}

/// The dialer a vm's `Class:open` calls: the egress check, the
/// handshake, a minted identity, and the same actor an upgrade gets,
/// spawned to outlive whatever opened it. Answers with the instance
/// name once the wire is up.
pub(crate) fn dialer_for(
    state: AppState,
    prepared: Arc<PreparedRevision>,
    logs: Option<LogPublisher>,
) -> Dialer {
    Arc::new(move |request| {
        let state = state.clone();
        let prepared = prepared.clone();
        let logs = logs.clone();
        Box::pin(async move {
            let scope = scope_egress(
                project_policy(&state, &prepared.script.project_id)
                    .await
                    .as_ref(),
            );
            let (wire, host) = tokio::time::timeout(DIAL_BUDGET, dial(&state, &request, &scope))
                .await
                .map_err(|_| "the handshake took too long.".to_owned())??;
            let name = uuid::Uuid::new_v4().simple().to_string()[..12].to_owned();
            let node = state
                .node_identity
                .read()
                .ok()
                .and_then(|guard| guard.clone())
                .unwrap_or_default();
            let about = actias_worker_core::extensions::sockets::About {
                connection_class: request.spec.name.clone(),
                direction: Some(actias_worker_core::connections::Direction::Outbound),
                peer: Some(host),
                project_id: prepared.script.project_id.clone(),
                script_id: prepared.script.id.clone(),
                opened_at_ms: actias_worker_core::extensions::objects::unix_now_ms(),
            };
            let permit = state
                .shares
                .connections
                .try_acquire(&prepared.script.project_id)
                .map_err(|_| TOO_MANY_CONNECTIONS.to_owned())?;
            let spawn = ConnectionSpawn {
                factory: vm_factory(state.clone(), prepared.clone(), logs),
                pending: actias_worker_core::extensions::sockets::PendingUpgrade {
                    class: request.spec.name.clone(),
                    name: name.clone(),
                    spec: request.spec,
                    seed: request.seed,
                },
                registry: state.connections.clone(),
                router: ObjectRouting::new(&state, prepared).as_router(),
                node,
                hibernate_after: state.connection_hibernate_after,
                gauges: state.connection_gauges.clone(),
                about,
                permit: Some(permit),
            };
            tokio::spawn(drive_wire(wire, spawn));
            Ok(name)
        })
    })
}

/// Longest a `Class:open` waits for the far side to complete the
/// handshake.
const DIAL_BUDGET: std::time::Duration = std::time::Duration::from_secs(10);

/// Opens the wire: the url through the egress policy, every address
/// the host resolves to checked before any is connected, then the
/// websocket handshake with the caller's headers and subprotocols.
async fn dial(
    state: &AppState,
    request: &actias_worker_core::extensions::sockets::DialRequest,
    scope: &ScopeEgress,
) -> Result<(Wire, String), String> {
    let url = url::Url::parse(&request.url).map_err(|_| "the url does not parse.".to_owned())?;
    let secure = match url.scheme() {
        "wss" => true,
        "ws" => false,
        _ => return Err("the url must start with ws:// or wss://.".to_owned()),
    };
    state
        .egress
        .policy
        .check_url(&url, Some(scope))
        .map_err(|denied| denied.to_string())?;
    let host = url
        .host_str()
        .ok_or_else(|| "the url has no host.".to_owned())?
        .to_owned();
    let port = url
        .port_or_known_default()
        .unwrap_or(if secure { 443 } else { 80 });
    // Resolved here rather than by the handshake, so a name pointing at
    // a private address is refused the way an http request's is.
    let addresses: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|_| format!("'{host}' does not resolve."))?
        .collect();
    if addresses.is_empty() {
        return Err(format!("'{host}' does not resolve."));
    }
    for address in &addresses {
        state
            .egress
            .policy
            .check_ip(address.ip())
            .map_err(|denied| denied.to_string())?;
    }
    let stream = tokio::net::TcpStream::connect(&addresses[..])
        .await
        .map_err(|error| format!("'{host}' refused the connection: {error}"))?;

    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut handshake = request
        .url
        .as_str()
        .into_client_request()
        .map_err(|error| format!("the url cannot be dialled: {error}"))?;
    for (name, value) in &request.headers {
        let name = tokio_tungstenite::tungstenite::http::HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| format!("'{name}' is not a header name."))?;
        let value = tokio_tungstenite::tungstenite::http::HeaderValue::from_str(value)
            .map_err(|_| format!("the value of '{name}' is not a header value."))?;
        handshake.headers_mut().insert(name, value);
    }
    if !request.protocols.is_empty() {
        let value = tokio_tungstenite::tungstenite::http::HeaderValue::from_str(
            &request.protocols.join(", "),
        )
        .map_err(|_| "the subprotocol list is not a header value.".to_owned())?;
        handshake
            .headers_mut()
            .insert("Sec-WebSocket-Protocol", value);
    }
    let (socket, _response) = tokio_tungstenite::client_async_tls(handshake, stream)
        .await
        .map_err(|error| format!("the handshake with '{host}' failed: {error}"))?;
    Ok((Wire::Outbound(Box::new(socket)), host))
}

/// The bridge between one live websocket and its connection actor:
/// client frames feed the same inbox edge deliveries use, handler
/// sends feed the wire, and the actor drives the declared handlers in
/// vms it builds from the factory. When either side ends, edges sever
/// (politely by the actor, or by the pump's deliver-or-prune for
/// whatever that missed) and the registry forgets the id.
/// Everything a connection needs besides its socket, gathered at the
/// upgrade or the dial and handed to the task that outlives it.
struct ConnectionSpawn {
    factory: actias_worker_core::connections::actor::VmFactory,
    pending: actias_worker_core::extensions::sockets::PendingUpgrade,
    registry: Arc<actias_worker_core::connections::ConnectionRegistry>,
    router: ObjectRouter,
    /// The node hosting this connection, so a publisher homed elsewhere
    /// knows where its events must travel.
    node: String,
    hibernate_after: Option<std::time::Duration>,
    gauges: Arc<actias_worker_core::connections::actor::ConnectionGauges>,
    about: actias_worker_core::extensions::sockets::About,
    /// The project's share of this node's connections, held for as long
    /// as the connection is registered.
    permit: Option<actias_worker_core::shares::Permit>,
}

/// The refusal when a project holds its share of this node's connections.
const TOO_MANY_CONNECTIONS: &str =
    "This project has too many open connections on this node; retry shortly.";

/// A live websocket, whichever side opened it. The bridge speaks in
/// text frames and closes; the two libraries' message types stay here.
enum Wire {
    Inbound(Box<axum::extract::ws::WebSocket>),
    Outbound(
        Box<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
        >,
    ),
}

/// What the bridge cares about in a received message.
enum Received {
    Text(String),
    /// A close frame, the peer going away, or a read error.
    Ended(Option<String>),
    Other,
}

impl Wire {
    async fn recv(&mut self) -> Received {
        match self {
            Self::Inbound(socket) => match socket.recv().await {
                Some(Ok(axum::extract::ws::Message::Text(text))) => {
                    Received::Text(text.to_string())
                }
                Some(Ok(axum::extract::ws::Message::Close(_))) | None => Received::Ended(None),
                Some(Err(error)) => Received::Ended(Some(error.to_string())),
                Some(Ok(_)) => Received::Other,
            },
            Self::Outbound(socket) => {
                use futures::StreamExt;
                use tokio_tungstenite::tungstenite::Message;
                match socket.next().await {
                    Some(Ok(Message::Text(text))) => Received::Text(text.to_string()),
                    Some(Ok(Message::Close(_))) | None => Received::Ended(None),
                    Some(Err(error)) => Received::Ended(Some(error.to_string())),
                    Some(Ok(_)) => Received::Other,
                }
            }
        }
    }

    async fn send_text(&mut self, text: String) -> Result<(), String> {
        match self {
            Self::Inbound(socket) => socket
                .send(axum::extract::ws::Message::Text(text.into()))
                .await
                .map_err(|error| error.to_string()),
            Self::Outbound(socket) => {
                use futures::SinkExt;
                socket
                    .send(tokio_tungstenite::tungstenite::Message::Text(text.into()))
                    .await
                    .map_err(|error| error.to_string())
            }
        }
    }

    async fn close(&mut self) {
        match self {
            Self::Inbound(socket) => {
                let _ = socket.send(axum::extract::ws::Message::Close(None)).await;
            }
            Self::Outbound(socket) => {
                use futures::SinkExt;
                let _ = socket
                    .send(tokio_tungstenite::tungstenite::Message::Close(None))
                    .await;
            }
        }
    }
}

async fn drive_wire(mut wire: Wire, spawn: ConnectionSpawn) {
    let ConnectionSpawn {
        factory,
        pending,
        registry,
        router,
        node,
        hibernate_after,
        gauges,
        about,
        permit,
    } = spawn;
    use actias_worker_core::connections::actor::ConnectionTask;
    use actias_worker_core::connections::{Closed, ClosedBy, InboxItem, OutboundFrame, inbox};
    use actias_worker_core::extensions::sockets::SockShared;

    let connection_id = format!("conn#{}", uuid::Uuid::new_v4().simple());
    let (inbox_tx, inbox_rx) = inbox();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<OutboundFrame>(64);

    let shared = SockShared::with_about(
        connection_id.clone(),
        node,
        pending.class.clone(),
        pending.name.clone(),
        out_tx,
        router,
        about,
    );
    registry.register_with(&connection_id, inbox_tx.clone(), shared.clone(), permit);

    // One task owns the socket, both directions: uplink text frames
    // decode into the inbox (the wire speaks json; anything else is
    // dropped), downlink frames encode out, and Close from either side
    // ends the wire. Why it ended is recorded for the close handler.
    let wire_shared = shared.clone();
    let wire_task = async move {
        loop {
            tokio::select! {
                incoming = wire.recv() => match incoming {
                    Received::Text(text) => {
                        if let Ok(data) = serde_json::from_str::<serde_json::Value>(&text)
                            && inbox_tx.push(InboxItem::Frame(data)).is_err()
                        {
                            wire_shared.record_closed(Closed { by: ClosedBy::Overflow, reason: None });
                            wire.close().await;
                            break;
                        }
                    }
                    Received::Ended(reason) => {
                        wire_shared.record_closed(Closed { by: ClosedBy::Peer, reason });
                        let _ = inbox_tx.push(InboxItem::Closed);
                        break;
                    }
                    Received::Other => {}
                },
                outgoing = out_rx.recv() => match outgoing {
                    Some(OutboundFrame::Json(value)) => {
                        if let Err(error) = wire.send_text(value.to_string()).await {
                            wire_shared.record_closed(Closed { by: ClosedBy::Peer, reason: Some(error) });
                            let _ = inbox_tx.push(InboxItem::Closed);
                            break;
                        }
                    }
                    Some(OutboundFrame::Close) | None => {
                        wire_shared.record_closed(Closed { by: ClosedBy::Program, reason: None });
                        wire.close().await;
                        let _ = inbox_tx.push(InboxItem::Closed);
                        break;
                    }
                },
            }
        }
    };

    let task = ConnectionTask::new(
        inbox_rx,
        shared,
        pending.spec,
        pending.seed,
        factory,
        hibernate_after,
        gauges,
    );
    let (_, outcome) = tokio::join!(wire_task, task.run());
    if let Err(error) = outcome {
        actias_common::tracing::debug!(%error, connection_id, "connection ended with an error");
    }
    registry.unregister(&connection_id);
}

/// State builders every worker test suite shares: clients that never
/// connect, so a test passes only when its path never needs a backend.
#[cfg(test)]
pub(crate) mod test_state {
    use super::*;

    /// Clients that never connect; reachable code paths must fail before
    /// using them or the test is wrong.
    fn lazy_clients() -> Clients {
        let channel = Channel::from_static("http://127.0.0.1:1").connect_lazy();
        Clients {
            script: ScriptServiceClient::new(actias_worker_core::plain_grpc(channel.clone())),
            kv: KvServiceClient::new(actias_worker_core::plain_grpc(channel)),
        }
    }

    /// Empty caches sized like production, so every lookup is a miss.
    pub(crate) fn empty_caches() -> WorkerCaches {
        WorkerCaches::new(Duration::from_secs(5), 64 * 1024 * 1024)
    }

    /// A guarded client with the production default policy.
    fn test_egress() -> EgressClient {
        EgressClient::new(actias_worker_core::egress::EgressPolicy::new([], false)).unwrap()
    }

    /// State over `caches` whose every client is unreachable, so a test
    /// passes only when its path never needs a backend.
    pub(crate) fn state_with(caches: WorkerCaches) -> AppState {
        AppState {
            clients: lazy_clients(),
            caches,
            blobs: unreachable_blobs(),
            egress: test_egress(),
            redis: None,
            secret_client: None,
            request_timeout: Duration::from_secs(5),
            guest_limits: GuestLimits {
                work: actias_worker_core::budget::DEFAULT_WORK_LIMIT,
                wall_secs: 10,
            },
            in_flight: Arc::default(),
            objects: Arc::default(),
            metrics: Arc::default(),
            replica_ttl: Duration::from_secs(30),
            membership_generation: Arc::default(),
            replica_store: crate::objects::replica::ReplicaStore::new(
                std::env::temp_dir().join(format!("actias-replicas-{}", uuid::Uuid::new_v4())),
                crate::objects::replica::SyncMode::Os,
                Duration::from_secs(1800),
            ),
            replica_count: 0,
            replica_quorum: 0,
            replica_ack: Duration::from_secs(2),
            vm_pool: crate::vm_pool::VmPool::new(0),
            node_address: "127.0.0.1:0".to_owned(),
            region: "local".to_owned(),
            shares: ScopeShares::unbounded(),
            rates: RateLimits::new(),
            ship_states: Arc::default(),
            peers: moka::future::Cache::new(100),
            holders: moka::future::Cache::builder()
                .max_capacity(200_000)
                .time_to_live(std::time::Duration::from_secs(15))
                .build(),
            node_addrs: moka::future::Cache::builder()
                .max_capacity(1_000)
                .time_to_live(std::time::Duration::from_secs(120))
                .build(),
            internal_token: "test-internal".to_owned(),
            connection_gauges: Arc::default(),
            connection_hibernate_after: Some(Duration::from_secs(300)),
            object_store: Arc::new(ObjectStore::new(
                crate::blob_cache::s3_client("http://127.0.0.1:1", "unused", "unused"),
                "unused".to_owned(),
                1024 * 1024,
                8,
            )),
            object_data_dir: std::env::temp_dir(),
            object_db_max_bytes: 64 * 1024 * 1024,
            ship_thresholds: crate::objects::store::ShipThresholds {
                rotate_bytes: 4096 * 1024,
                rotate_fraction: 0.125,
                max_segments: 64,
            },
            ack_gate: Duration::from_millis(10_000),
            ship_gauges: Arc::default(),
            ship_limits: crate::objects::shipper::ShipLimits::new(
                actias_worker_core::shares::Pool::new("ships", 32, 0.0),
                8,
            ),
            directory_sync: crate::directory::sync::DirectorySyncer::new(
                Arc::new(|_, _, _| Box::pin(async { Ok(()) })),
                std::env::temp_dir(),
                Arc::default(),
            ),
            directory_eval_budget_ms: 5,
            directory_gauges: Arc::default(),
            reader_membership: moka::future::Cache::builder()
                .time_to_live(Duration::from_secs(10))
                .build(),
            directory_overlays: Arc::default(),
            directory_recomputed: Arc::default(),
            admit_refusals: moka::future::Cache::builder()
                .max_capacity(100_000)
                .time_to_live(std::time::Duration::from_secs(5))
                .build(),
            armed_crons: Arc::default(),
            object_idle_after: Duration::from_secs(300),
            queue_policy: Default::default(),
            node_identity: Arc::default(),
            shippers: Arc::default(),
            registry: NodeRegistryServiceClient::new(actias_worker_core::plain_grpc(
                Channel::from_static("http://127.0.0.1:1").connect_lazy(),
            )),
            base_domain: None,
            connections: Arc::default(),
        }
    }

    /// A blob cache whose store is unreachable; anything hitting it fails.
    fn unreachable_blobs() -> BlobCache {
        BlobCache::new(crate::blob_cache::BlobCacheConfig {
            endpoint: "http://127.0.0.1:1".to_owned(),
            access_key: "unused".to_owned(),
            secret_key: "unused".to_owned(),
            bucket: "unused".to_owned(),
            cache_bytes: 1024 * 1024,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::test_state::{empty_caches, state_with};
    use super::*;
    use axum::body::to_bytes;
    use tower::ServiceExt;

    #[test]
    fn route_reads_the_first_path_segment_as_the_published_script() {
        for path in ["/my-script", "/my-script/users/1", "/my-script/"] {
            assert_eq!(
                route_by_path(path),
                Some(Route::Published {
                    identifier: "my-script"
                }),
                "path {path:?}"
            );
        }
    }

    #[test]
    fn route_is_absent_when_the_path_names_nothing() {
        // A bare root addresses the worker itself, not a script, so it must not
        // reach the script service with an empty name.
        assert_eq!(route_by_path("/"), None);
        assert_eq!(route_by_path(""), None);
        assert_eq!(route_by_path("//"), None);
    }

    #[test]
    fn route_reads_the_live_prefix_as_a_session() {
        assert_eq!(
            route_by_path("/_live/my-script/sess-1/users"),
            Some(Route::Live {
                identifier: "my-script",
                session: "sess-1"
            })
        );
    }

    #[test]
    fn route_rejects_a_live_path_missing_its_session() {
        // Half a live address must not fall through and run something else.
        assert_eq!(route_by_path("/_live"), None);
        assert_eq!(route_by_path("/_live/my-script"), None);
        assert_eq!(route_by_path("/_live/my-script/"), None);
    }

    #[tokio::test]
    async fn internal_error_response_reveals_only_a_correlation_id() {
        let response = internal_error_response(&anyhow::anyhow!(
            "connection to postgres://actias:hunter2@db:5432/actias refused"
        ));

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();

        assert!(!body.contains("postgres"), "leaked the cause: {body}");
        assert!(!body.contains("hunter2"), "leaked the cause: {body}");
        assert!(!body.contains("refused"), "leaked the cause: {body}");
        assert!(body.contains("Correlation ID"), "unusable message: {body}");
    }

    #[tokio::test]
    async fn a_refused_share_answers_429_with_when_to_retry() {
        let pool = actias_worker_core::shares::Pool::new("requests", 1, 0.0);
        let _held = pool.try_acquire("proj").expect("granted");
        let refused = pool.try_acquire("proj").expect_err("over its share");

        let response = refused_response(&refused);

        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok()),
            Some("1")
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_oversized_body_is_rejected_before_any_backend_call() {
        // 1 KiB cap; the clients are unconnectable, so reaching them would
        // turn this 413 into a 500 and fail the assertion.
        let app = router(state_with(empty_caches()), 1024);

        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/some-script/upload")
            .body(Body::from(vec![0u8; 4096]))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_bare_root_is_a_404_without_any_backend_call() {
        let app = router(state_with(empty_caches()), 1024);

        let request = axum::http::Request::builder()
            .uri("/")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    /// Hash the fixture asset claims, standing in for its blake3.
    const ASSET_HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    /// Caches holding `cached-script` fully resolved: pointer and prepared
    /// revision both warm, so the published path needs no backend at all.
    async fn caches_with_cached_script() -> WorkerCaches {
        use actias_worker_core::proto::bundle::{Bundle, File};

        let caches = empty_caches();

        let script = Script {
            id: "script-1".to_owned(),
            project_id: "project-1".to_owned(),
            public_identifier: "cached-script".to_owned(),
            current_revision_id: Some("revision-1".to_owned()),
            ..Default::default()
        };

        let source = br#"on "fetch" (function(request)
            return { body = "served from cache" }
        end)"#;

        let revision = Revision {
            bundle: Some(Bundle {
                entry_point: "main.lua".to_owned(),
                files: vec![
                    File {
                        file_path: "main.lua".to_owned(),
                        content: source.to_vec(),
                        ..Default::default()
                    },
                    File {
                        file_path: "motd.txt".to_owned(),
                        content: b"static bytes".to_vec(),
                        content_type: "text/plain; charset=utf-8".to_owned(),
                        kind: actias_worker_core::proto::bundle::FileKind::Asset as i32,
                        hash: ASSET_HASH.to_owned(),
                        ..Default::default()
                    },
                ],
            }),
            ..Default::default()
        };

        caches
            .pointers
            .insert("cached-script".to_owned(), script.clone())
            .await;
        caches
            .revisions
            .insert(
                "revision-1".to_owned(),
                Arc::new(PreparedRevision::prepare(script, revision).unwrap()),
            )
            .await;

        caches
    }

    /// Caches holding `cached-ws`, a script whose fetch handler
    /// upgrades websocket requests and serves plain ones normally.
    async fn caches_with_upgrading_script() -> WorkerCaches {
        use actias_worker_core::proto::bundle::{Bundle, File};

        let caches = empty_caches();
        let script = Script {
            id: "script-ws".to_owned(),
            project_id: "project-1".to_owned(),
            public_identifier: "cached-ws".to_owned(),
            current_revision_id: Some("revision-ws".to_owned()),
            ..Default::default()
        };
        let source = br#"
            local U = object "U" {}
            local Echo = connection "Echo" {
                frame = function(conn, data)
                    conn:send({ echo = data.hello })
                    conn:close()
                end,
            }
            on "fetch" (function(request)
                if request.upgrade then
                    return request:upgrade(Echo, U("solo"))
                end
                return { body = "no ws" }
            end)
        "#;
        let revision = Revision {
            bundle: Some(Bundle {
                entry_point: "main.lua".to_owned(),
                files: vec![File {
                    file_path: "main.lua".to_owned(),
                    content: source.to_vec(),
                    ..Default::default()
                }],
            }),
            ..Default::default()
        };
        caches
            .pointers
            .insert("cached-ws".to_owned(), script.clone())
            .await;
        caches
            .revisions
            .insert(
                "revision-ws".to_owned(),
                Arc::new(PreparedRevision::prepare(script, revision).unwrap()),
            )
            .await;
        caches
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_plain_request_to_an_upgrading_script_stays_http() {
        let app = router(
            state_with(caches_with_upgrading_script().await),
            1024 * 1024,
        );
        let request = axum::http::Request::builder()
            .uri("/cached-ws/")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"no ws");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_websocket_handshake_reaches_101_through_the_fetch_handler() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // A real connection on purpose: the upgrade needs hyper's
        // OnUpgrade extension, which tower::oneshot never carries.
        let app = router(
            state_with(caches_with_upgrading_script().await),
            1024 * 1024,
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        stream
            .write_all(
                b"GET /cached-ws/ws HTTP/1.1\r\n\
                  Host: localhost\r\n\
                  Connection: Upgrade\r\n\
                  Upgrade: websocket\r\n\
                  Sec-WebSocket-Version: 13\r\n\
                  Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
                  \r\n",
            )
            .await
            .unwrap();

        let mut head = Vec::new();
        let mut buffer = [0u8; 1024];
        loop {
            let n =
                tokio::time::timeout(std::time::Duration::from_secs(5), stream.read(&mut buffer))
                    .await
                    .expect("the handshake answers")
                    .unwrap();
            assert!(n > 0, "the server closed before answering");
            head.extend_from_slice(&buffer[..n]);
            if head.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        let head = String::from_utf8_lossy(&head);
        assert!(
            head.starts_with("HTTP/1.1 101"),
            "the handler's upgrade becomes the handshake: {head}"
        );
        assert!(
            head.to_lowercase().contains("sec-websocket-accept"),
            "the accept key rides the 101: {head}"
        );

        // Full circle over the real wire: one masked client text frame
        // in, the program's echo frame out. Hand-rolled framing so no
        // client library enters the tree.
        let payload = br#"{"hello":"actias"}"#;
        let mask = [0x11u8, 0x22, 0x33, 0x44];
        let mut frame = vec![0x81u8, 0x80 | payload.len() as u8];
        frame.extend_from_slice(&mask);
        frame.extend(
            payload
                .iter()
                .enumerate()
                .map(|(at, byte)| byte ^ mask[at % 4]),
        );
        stream.write_all(&frame).await.unwrap();

        let mut reply = Vec::new();
        loop {
            let n =
                tokio::time::timeout(std::time::Duration::from_secs(5), stream.read(&mut buffer))
                    .await
                    .expect("the echo answers")
                    .unwrap();
            assert!(n > 0, "the server closed before echoing");
            reply.extend_from_slice(&buffer[..n]);
            if reply.len() >= 2 && reply.len() >= 2 + (reply[1] & 0x7f) as usize {
                break;
            }
        }
        assert_eq!(reply[0], 0x81, "one unmasked text frame comes back");
        let length = (reply[1] & 0x7f) as usize;
        let echoed: serde_json::Value =
            serde_json::from_slice(&reply[2..2 + length]).expect("the frame is json");
        assert_eq!(echoed["echo"], "actias", "the program echoed the field");

        server.abort();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_cached_revision_serves_without_any_backend_call() {
        // Both caches are seeded by hand and the clients are unconnectable,
        // so this 200 proves a warm request spends zero grpc calls.
        let app = router(state_with(caches_with_cached_script().await), 1024);

        let request = axum::http::Request::builder()
            .uri("/cached-script/")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"served from cache");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_live_request_never_serves_the_published_cache() {
        // The same script is fully warm in both caches, but a live request
        // must fetch the session's current bundle; with unconnectable
        // clients that fetch fails, so a 200 here means the live path served
        // the published revision, which is exactly the bug this guards.
        let app = router(state_with(caches_with_cached_script().await), 1024);

        let request = axum::http::Request::builder()
            .uri("/_live/cached-script/some-session/")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn route_by_path_reads_the_rev_prefix_as_a_preview() {
        assert_eq!(
            route_by_path("/_rev/my-script/rev-1/users"),
            Some(Route::Revision {
                identifier: "my-script",
                revision: "rev-1"
            })
        );
        // Half a preview address must not fall through to something else.
        assert_eq!(route_by_path("/_rev/my-script"), None);
    }

    #[test]
    fn route_by_host_reads_the_label_under_the_base_domain() {
        let base = "scripts.example.com";

        assert_eq!(
            route_by_host("my-script.scripts.example.com", base),
            Some(Route::Published {
                identifier: "my-script"
            })
        );
        assert_eq!(
            route_by_host("my-script.scripts.example.com:8443", base),
            Some(Route::Published {
                identifier: "my-script"
            })
        );
        assert_eq!(
            route_by_host("my-script--live-sess-1.scripts.example.com", base),
            Some(Route::Live {
                identifier: "my-script",
                session: "sess-1"
            })
        );
        assert_eq!(
            route_by_host("my-script--r-rev-1.scripts.example.com", base),
            Some(Route::Revision {
                identifier: "my-script",
                revision: "rev-1"
            })
        );
    }

    #[test]
    fn route_by_host_ignores_hosts_outside_the_base_domain() {
        let base = "scripts.example.com";

        // The bare base, a foreign domain, a suffix that is not a label
        // boundary, and a nested label all fall back to path routing.
        assert_eq!(route_by_host("scripts.example.com", base), None);
        assert_eq!(route_by_host("example.com", base), None);
        assert_eq!(route_by_host("evilscripts.example.com", base), None);
        assert_eq!(route_by_host("a.b.scripts.example.com", base), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_host_under_the_base_domain_routes_by_subdomain() {
        let mut state = state_with(caches_with_cached_script().await);
        state.base_domain = Some("scripts.local".to_owned());
        let app = router(state, 1024);

        // The path carries no identifier at all: the host alone selects the
        // script, and the script sees the bare root.
        let request = axum::http::Request::builder()
            .uri("/")
            .header("host", "cached-script.scripts.local")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"served from cache");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_preview_host_serves_a_cached_revision_without_a_backend() {
        let mut state = state_with(caches_with_cached_script().await);
        state.base_domain = Some("scripts.local".to_owned());
        let app = router(state, 1024);

        // revision-1 is warm in the revision cache and the clients are
        // unconnectable, so a 200 proves the preview route reuses the same
        // immutable cache the published path fills.
        let request = axum::http::Request::builder()
            .uri("/")
            .header("host", "cached-script--r-revision-1.scripts.local")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"served from cache");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_asset_serves_through_the_subdomain_route() {
        let mut state = state_with(caches_with_cached_script().await);
        state.base_domain = Some("scripts.local".to_owned());
        let app = router(state, 1024);

        let request = axum::http::Request::builder()
            .uri("/motd.txt")
            .header("host", "cached-script.scripts.local")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"static bytes");
    }

    #[test]
    fn route_parsers_read_alias_forms() {
        assert_eq!(
            route_by_path("/_alias/my-script/staging/users"),
            Some(Route::Aliased {
                identifier: "my-script",
                alias: "staging"
            })
        );
        assert_eq!(
            route_by_host(
                "my-script--staging.scripts.example.com",
                "scripts.example.com"
            ),
            Some(Route::Aliased {
                identifier: "my-script",
                alias: "staging"
            })
        );
        // The reserved markers still win over the generic alias split.
        assert_eq!(
            route_by_host(
                "my-script--live-s1.scripts.example.com",
                "scripts.example.com"
            ),
            Some(Route::Live {
                identifier: "my-script",
                session: "s1"
            })
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_alias_serves_its_revision_through_the_caches() {
        let mut state = state_with(caches_with_cached_script().await);
        state.base_domain = Some("scripts.local".to_owned());

        // The alias pointer is warm and names the cached revision, so the
        // whole chain resolves without any backend.
        state
            .caches
            .aliases
            .insert("script-1/staging".to_owned(), "revision-1".to_owned())
            .await;

        let app = router(state, 1024);

        let request = axum::http::Request::builder()
            .uri("/")
            .header("host", "cached-script--staging.scripts.local")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"served from cache");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_served_request_shows_up_in_the_metrics() {
        let app = router(state_with(caches_with_cached_script().await), 1024);

        let request = axum::http::Request::builder()
            .uri("/cached-script/")
            .body(Body::empty())
            .unwrap();
        app.clone().oneshot(request).await.unwrap();

        let scrape = axum::http::Request::builder()
            .uri("/_metrics")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(scrape).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            text.contains(r#"actias_requests_total{project="project-1",script="cached-script"} 1"#),
            "{text}"
        );
        assert!(text.contains("actias_objects_resident 0"), "{text}");
    }

    #[test]
    fn a_backend_not_found_reads_as_target_absent() {
        // The loader wraps the grpc status in anyhow; classification walks
        // the chain, so the 404 mapping survives wrapping.
        let absent = anyhow::Error::from(tonic::Status::not_found("no script by that name"));
        assert!(target_absent(&absent));

        // Infrastructure failing must never read as "no such script": a
        // dead backend is an incident, not a visitor's typo.
        let broken = anyhow::Error::from(tonic::Status::unavailable("backend down"));
        assert!(!target_absent(&broken));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_in_flight_gauge_returns_to_zero_after_a_request() {
        let state = state_with(caches_with_cached_script().await);
        let gauge = state.in_flight.clone();
        let app = router(state, 1024);

        let request = axum::http::Request::builder()
            .uri("/cached-script/")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(gauge.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn the_script_relative_path_keeps_a_trailing_slash() {
        // The trailing slash selects a directory's index asset, so the
        // routing strip must not eat it.
        assert_eq!(script_relative_path("/my-script", 1), "");
        assert_eq!(script_relative_path("/my-script/", 1), "");
        assert_eq!(script_relative_path("/my-script/motd.txt", 1), "motd.txt");
        assert_eq!(script_relative_path("/my-script/docs/", 1), "docs/");
        assert_eq!(script_relative_path("/_live/s/sess/a/b", 3), "a/b");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_get_naming_an_asset_is_served_without_a_vm() {
        let app = router(state_with(caches_with_cached_script().await), 1024);

        let request = axum::http::Request::builder()
            .uri("/cached-script/motd.txt")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("content-type").unwrap(),
            "text/plain; charset=utf-8"
        );
        assert_eq!(
            response.headers().get("etag").unwrap(),
            &format!("\"{ASSET_HASH}\"")
        );

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"static bytes");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_held_etag_revalidates_to_a_304() {
        let app = router(state_with(caches_with_cached_script().await), 1024);

        let request = axum::http::Request::builder()
            .uri("/cached-script/motd.txt")
            .header("if-none-match", format!("\"{ASSET_HASH}\""))
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(body.is_empty(), "a 304 must not carry the bytes");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_script_root_missing_its_slash_redirects_to_the_canonical_form() {
        let app = router(state_with(caches_with_cached_script().await), 1024);

        let request = axum::http::Request::builder()
            .uri("/cached-script?tab=docs")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
        assert_eq!(
            response.headers()[axum::http::header::LOCATION],
            "/cached-script/?tab=docs"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_post_to_an_asset_path_reaches_the_script() {
        // Assets answer GET and HEAD only; anything else belongs to the
        // script's own routing.
        let app = router(state_with(caches_with_cached_script().await), 1024);

        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/cached-script/motd.txt")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"served from cache");
    }
}
