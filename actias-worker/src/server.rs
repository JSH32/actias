use std::sync::Arc;
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
use std::path;
use tonic::transport::Channel;

use crate::egress::EgressClient;
use crate::extensions;
use crate::extensions::http::Request as LuaRequest;
use crate::proto::kv_service::kv_service_client::KvServiceClient;
use crate::proto::script_service::FindScriptRequest;
use crate::proto::script_service::GetRevisionRequest;
use crate::proto::script_service::LiveScriptSession;
use crate::proto::script_service::Revision;
use crate::proto::script_service::Script;
use crate::proto::script_service::find_script_request::Query;
use crate::proto::script_service::script_service_client::ScriptServiceClient;
use crate::runtime::{ActiasRuntime, PreparedRevision};

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
}

impl WorkerCaches {
    pub fn new(pointer_ttl: Duration, revision_cache_bytes: u64) -> Self {
        Self {
            pointers: moka::future::Cache::builder()
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
pub fn router(
    clients: Clients,
    caches: WorkerCaches,
    egress: EgressClient,
    max_body_bytes: usize,
    request_timeout: Duration,
) -> Router {
    Router::new()
        .fallback(handle)
        .layer(DefaultBodyLimit::max(max_body_bytes))
        .with_state(AppState {
            clients,
            caches,
            egress,
            request_timeout,
        })
}

#[derive(Clone)]
struct AppState {
    clients: Clients,
    caches: WorkerCaches,
    egress: EgressClient,
    request_timeout: Duration,
}

/// Where a request path points.
#[derive(Debug, PartialEq)]
enum Route<'a> {
    /// The current published revision of a script.
    Published { identifier: &'a str },
    /// One live development session's working tree.
    Live {
        identifier: &'a str,
        session: &'a str,
    },
}

impl Route<'_> {
    /// Path segments the route consumed, which the script never sees.
    fn consumed_segments(&self) -> usize {
        match self {
            Route::Published { .. } => 1,
            Route::Live { .. } => 3,
        }
    }
}

/// Extracts the target a request path addresses.
///
/// The first segment selects the script, so `/my-script/users` runs the
/// published `my-script` and hands it `/users`. The `_live` prefix is
/// reserved: `/_live/my-script/<session>/users` runs that script's live
/// session instead, so no published script named `_live` is reachable.
/// A path missing any needed segment, such as `/`, addresses nothing.
fn route(path: &str) -> Option<Route<'_>> {
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());

    match segments.next()? {
        "_live" => Some(Route::Live {
            identifier: segments.next()?,
            session: segments.next()?,
        }),
        identifier => Some(Route::Published { identifier }),
    }
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

/// Resolves the script, runs it, and shapes its response.
async fn run_script(state: AppState, request: axum::extract::Request) -> anyhow::Result<Response> {
    // DefaultBodyLimit only takes effect through extractors, so the body is
    // wrapped explicitly; without this the cap silently would not apply.
    use axum::RequestExt;
    let request = request.with_limited_body();

    let (parts, body) = request.into_parts();

    let Some(route) = route(parts.uri.path()) else {
        return Ok(text_response(StatusCode::NOT_FOUND, "Invalid script."));
    };
    let consumed_segments = route.consumed_segments();
    let (identifier, live_session) = match route {
        Route::Published { identifier } => (identifier.to_owned(), None),
        Route::Live {
            identifier,
            session,
        } => (identifier.to_owned(), Some(session.to_owned())),
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

    let prepared = match live_session {
        // A live session is the developer's working tree, updated on every
        // save; serving it stale defeats its purpose, so nothing about it is
        // cached and every request fetches the session's current bundle.
        Some(session_id) => {
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
                    bundle: Some(live.bundle),
                    ..Default::default()
                },
            )?)
        }
        None => {
            let Some(revision_id) = script.current_revision_id.clone() else {
                return Ok(text_response(
                    StatusCode::NOT_FOUND,
                    "Script did not have a revision.",
                ));
            };

            state
                .caches
                .revisions
                .try_get_with(revision_id.clone(), {
                    let mut client = state.clients.script.clone();
                    async move {
                        let revision = client
                            .get_revision(GetRevisionRequest {
                                id: revision_id,
                                with_bundle: true,
                            })
                            .await?
                            .into_inner();

                        Ok::<_, anyhow::Error>(Arc::new(PreparedRevision::prepare(
                            script, revision,
                        )?))
                    }
                })
                .await
                .map_err(cache_load_error)?
        }
    };

    // Create a context URI without the routing segments, used for better
    // routing inside the script. The +1 skips the leading "/" component.
    let old_uri = &parts.uri;
    let path = path::Path::new(old_uri.path());
    let without_identifier: path::PathBuf = path.iter().skip(1 + consumed_segments).collect();
    let mut context_uri = Uri::builder().path_and_query(format!(
        "/{}{}",
        without_identifier.as_path().to_str().unwrap_or(""),
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

    let lua = ActiasRuntime::new(prepared, kv_client, state.egress.clone(), Some(10)).await?;

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
        EgressClient::new(crate::egress::EgressPolicy::new([], false)).unwrap()
    }

    #[test]
    fn route_reads_the_first_path_segment_as_the_published_script() {
        for path in ["/my-script", "/my-script/users/1", "/my-script/"] {
            assert_eq!(
                route(path),
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
        assert_eq!(route("/"), None);
        assert_eq!(route(""), None);
        assert_eq!(route("//"), None);
    }

    #[test]
    fn route_reads_the_live_prefix_as_a_session() {
        assert_eq!(
            route("/_live/my-script/sess-1/users"),
            Some(Route::Live {
                identifier: "my-script",
                session: "sess-1"
            })
        );
    }

    #[test]
    fn route_rejects_a_live_path_missing_its_session() {
        // Half a live address must not fall through and run something else.
        assert_eq!(route("/_live"), None);
        assert_eq!(route("/_live/my-script"), None);
        assert_eq!(route("/_live/my-script/"), None);
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
        let app = router(
            lazy_clients(),
            empty_caches(),
            test_egress(),
            1024,
            Duration::from_secs(5),
        );

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
        let app = router(
            lazy_clients(),
            empty_caches(),
            test_egress(),
            1024,
            Duration::from_secs(5),
        );

        let request = axum::http::Request::builder()
            .uri("/")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    /// Caches holding `cached-script` fully resolved: pointer and prepared
    /// revision both warm, so the published path needs no backend at all.
    async fn caches_with_cached_script() -> WorkerCaches {
        use crate::proto::bundle::{Bundle, File};

        let caches = empty_caches();

        let script = Script {
            id: "script-1".to_owned(),
            project_id: "project-1".to_owned(),
            public_identifier: "cached-script".to_owned(),
            current_revision_id: Some("revision-1".to_owned()),
            ..Default::default()
        };

        let source = br#"add_event_listener("fetch", function(request)
            return { body = "served from cache" }
        end)"#;

        let revision = Revision {
            bundle: Some(Bundle {
                entry_point: "main.lua".to_owned(),
                files: vec![File {
                    revision_id: "revision-1".to_owned(),
                    file_name: "main.lua".to_owned(),
                    file_path: "main.lua".to_owned(),
                    content: source.to_vec(),
                }],
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
        let app = router(
            lazy_clients(),
            caches_with_cached_script().await,
            test_egress(),
            1024,
            Duration::from_secs(5),
        );

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
        let app = router(
            lazy_clients(),
            caches_with_cached_script().await,
            test_egress(),
            1024,
            Duration::from_secs(5),
        );

        let request = axum::http::Request::builder()
            .uri("/_live/cached-script/some-session/")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
