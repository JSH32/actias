use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use actias_common::tracing::Level;
use actias_common::tracing::{error, span};
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
use actias_worker_core::extensions;
use actias_worker_core::extensions::http::Request as LuaRequest;
use actias_worker_core::extensions::log::LogPublisher;
use actias_worker_core::extensions::objects::{ObjectRouter, ObjectTarget};
use actias_worker_core::objects::ObjectHost;
use actias_worker_core::proto::bundle::File;
use actias_worker_core::proto::kv_service::kv_service_client::KvServiceClient;
use actias_worker_core::proto::script_service::FindScriptRequest;
use actias_worker_core::proto::script_service::GetAliasRequest;
use actias_worker_core::proto::script_service::GetRevisionRequest;
use actias_worker_core::proto::script_service::LiveScriptSession;
use actias_worker_core::proto::script_service::Revision;
use actias_worker_core::proto::script_service::Script;
use actias_worker_core::proto::script_service::find_script_request::Query;
use actias_worker_core::proto::script_service::script_service_client::ScriptServiceClient;
use actias_worker_core::runtime::{ActiasRuntime, PreparedRevision};

use crate::blob_cache::BlobCache;

/// The service clients every request handler needs.
#[derive(Clone)]
pub struct Clients {
    pub script: ScriptServiceClient<Channel>,
    pub kv: KvServiceClient<Channel>,
}

/// Hot-path caches shared across requests.
///
/// The pointer cache maps a public identifier to its script row and expires
/// quickly, so a publish propagates within the ttl. The revision cache holds
/// prepared revisions and is bounded by bytes rather than time, because a
/// revision is immutable; only eviction pressure should drop one.
#[derive(Clone)]
pub struct WorkerCaches {
    pointers: moka::future::Cache<String, Script>,
    revisions: moka::future::Cache<String, Arc<PreparedRevision>>,
    /// Alias pointers (`script_id/name` to revision id); mutable like the
    /// script pointer, so it expires on the same ttl.
    aliases: moka::future::Cache<String, String>,
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
    pub secrets_key: Option<Arc<[u8; actias_worker_core::extensions::secrets::KEY_LEN]>>,
    pub request_timeout: Duration,
    /// Requests currently executing; the heartbeat reports it as load.
    pub in_flight: Arc<AtomicU32>,
    /// Live durable objects on this node, one pinned vm each.
    pub objects: Arc<ObjectHost>,
    /// Domain subdomain routing hangs off; [`None`] leaves only the path
    /// forms.
    pub base_domain: Option<String>,
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
const FOREIGN_REVISION: &str = "the revision does not belong to this script";

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
/// cached, so an unknown identifier costs a lookup every time.
async fn resolve_script(
    caches: &WorkerCaches,
    client: &ScriptServiceClient<Channel>,
    identifier: String,
) -> anyhow::Result<Script> {
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
        .map_err(cache_load_error)
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

/// Handles every inbound request by running the addressed script.
async fn handle(State(state): State<AppState>, request: axum::extract::Request) -> Response {
    let span = span!(Level::DEBUG, "lua_http_request");
    let _enter = span.enter();
    let _in_flight = InFlight::enter(&state.in_flight);

    let deadline = state.request_timeout;
    let result = tokio::time::timeout(deadline, run_script(state, request)).await;

    match result {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => internal_error_response(&error),
        Err(_elapsed) => text_response(
            StatusCode::GATEWAY_TIMEOUT,
            "Script did not respond in time.",
        ),
    }
}

/// One revision prepared through the cache: the manifest travels over
/// grpc, the bytes come from the blob store by hash, and the compiled
/// result is shared by every request that runs it.
async fn cached_revision(
    state: &AppState,
    script: Script,
    revision_id: String,
) -> Result<Arc<PreparedRevision>, Arc<anyhow::Error>> {
    state
        .caches
        .revisions
        .try_get_with(revision_id.clone(), {
            let mut client = state.clients.script.clone();
            let blobs = state.blobs.clone();
            async move {
                let mut revision = client
                    .get_revision(GetRevisionRequest {
                        id: revision_id,
                        with_bundle: true,
                        manifest_only: true,
                    })
                    .await?
                    .into_inner();

                // A preview could name any revision; only this script's may
                // serve under its identifier. Failed loads are not cached.
                if revision.script_id != script.id {
                    anyhow::bail!(FOREIGN_REVISION);
                }

                if let Some(bundle) = revision.bundle.as_mut() {
                    for file in &mut bundle.files {
                        if file.content.is_empty() && !file.hash.is_empty() {
                            file.content = blobs.get(&file.hash).await?.as_ref().clone();
                        }
                    }
                }

                Ok::<_, anyhow::Error>(Arc::new(PreparedRevision::prepare(script, revision)?))
            }
        })
        .await
}

/// How method calls on object handles leave a request vm: resolve the
/// pinned vm (spawning it from the same prepared revision on first touch)
/// and push one mailbox message.
///
/// The vm-cache key carries the revision id, so a republish gets fresh
/// code on first touch; in-memory state resets with it, until durable
/// storage lands underneath. Pinned vms get no router of their own yet:
/// object-to-object calls are refused rather than allowed to deadlock on
/// each other's mailboxes.
fn object_router(state: &AppState, prepared: Arc<PreparedRevision>) -> ObjectRouter {
    let host = state.objects.clone();
    let kv = state.clients.kv.clone();
    let egress = state.egress.clone();
    let secrets_key = state.secrets_key.clone();
    let redis = state.redis.clone();

    Arc::new(move |target: ObjectTarget| {
        let host = host.clone();
        let kv = kv.clone();
        let egress = egress.clone();
        let secrets_key = secrets_key.clone();
        let redis = redis.clone();
        let prepared = prepared.clone();

        Box::pin(async move {
            let key = format!(
                "{}/{}/{}/{}",
                prepared.script.id, prepared.revision_id, target.class, target.name
            );

            let handle = host
                .get_or_spawn(&key, || async {
                    // Object logs join the script's production channel, so
                    // `actias tail` sees them like any handler line.
                    let logs = redis.map(|connection| {
                        LogPublisher::new(connection, script_log_channel(&prepared.script.id))
                    });

                    ActiasRuntime::new(prepared.clone(), kv, egress, logs, secrets_key, None).await
                })
                .await
                .map_err(|e| e.to_string())?;

            handle
                .call(
                    "__dispatch",
                    serde_json::json!({
                        "class": target.class,
                        "method": target.method,
                        "args": target.arguments,
                    }),
                )
                .await
                .map_err(|e| e.to_string())
        })
    })
}

/// Resolves the script, runs it, and shapes its response.
async fn run_script(state: AppState, request: axum::extract::Request) -> anyhow::Result<Response> {
    // DefaultBodyLimit only takes effect through extractors, so the body is
    // wrapped explicitly; without this the cap silently would not apply.
    use axum::RequestExt;
    let request = request.with_limited_body();

    let (parts, body) = request.into_parts();

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

    let script = resolve_script(&state.caches, &state.clients.script, identifier).await?;

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
    // directly on the async executor; the old block_in_place/LocalSet dance
    // died with mlua 0.9.
    let kv_client = state.clients.kv.clone();

    let router = object_router(&state, prepared.clone());

    let lua = ActiasRuntime::new(
        prepared,
        kv_client,
        state.egress.clone(),
        logs,
        state.secrets_key.clone(),
        Some(10),
    )
    .await?;
    lua.set_app_data::<ObjectRouter>(router);

    let listener = lua.listener(ActiasRuntime::FETCH_EVENT)?;

    lua.start_timer();

    let value: mlua::Value = listener.call_async(lua.to_value(&lua_request)?).await?;
    let lua_response: extensions::http::Response = lua.from_value(value)?;

    lua_response_into_response(lua_response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use tower::ServiceExt;

    /// Clients that never connect; reachable code paths must fail before
    /// using them or the test is wrong.
    fn lazy_clients() -> Clients {
        let channel = Channel::from_static("http://127.0.0.1:1").connect_lazy();
        Clients {
            script: ScriptServiceClient::new(channel.clone()),
            kv: KvServiceClient::new(channel),
        }
    }

    /// Empty caches sized like production, so every lookup is a miss.
    fn empty_caches() -> WorkerCaches {
        WorkerCaches::new(Duration::from_secs(5), 64 * 1024 * 1024)
    }

    /// A guarded client with the production default policy.
    fn test_egress() -> EgressClient {
        EgressClient::new(actias_worker_core::egress::EgressPolicy::new([], false)).unwrap()
    }

    /// State over `caches` whose every client is unreachable, so a test
    /// passes only when its path never needs a backend.
    fn state_with(caches: WorkerCaches) -> AppState {
        AppState {
            clients: lazy_clients(),
            caches,
            blobs: unreachable_blobs(),
            egress: test_egress(),
            redis: None,
            secrets_key: None,
            request_timeout: Duration::from_secs(5),
            in_flight: Arc::default(),
            objects: Arc::default(),
            base_domain: None,
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
