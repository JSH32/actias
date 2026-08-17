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
use tokio::runtime::Handle;
use tokio::task;
use tonic::transport::Channel;

use crate::extensions;
use crate::extensions::http::Request as LuaRequest;
use crate::proto::kv_service::kv_service_client::KvServiceClient;
use crate::proto::script_service::FindScriptRequest;
use crate::proto::script_service::GetRevisionRequest;
use crate::proto::script_service::find_script_request::Query;
use crate::proto::script_service::script_service_client::ScriptServiceClient;
use crate::runtime::ActiasRuntime;

/// The service clients every request handler needs.
#[derive(Clone)]
pub struct Clients {
    pub script: ScriptServiceClient<Channel>,
    pub kv: KvServiceClient<Channel>,
}

/// Builds the worker's http surface: every path and method funnels into the
/// script handler, bodies are capped, and the whole request carries a
/// deadline.
pub fn router(clients: Clients, max_body_bytes: usize, request_timeout: Duration) -> Router {
    Router::new()
        .fallback(handle)
        .layer(DefaultBodyLimit::max(max_body_bytes))
        .with_state(AppState {
            clients,
            request_timeout,
        })
}

#[derive(Clone)]
struct AppState {
    clients: Clients,
    request_timeout: Duration,
}

/// Extracts the script identifier from a request path.
///
/// The first path segment selects the script, so `/my-script/users` runs
/// `my-script` and hands it `/users`. A path with no first segment, such as
/// `/`, addresses no script at all.
fn script_identifier(path: &str) -> Option<&str> {
    path.split('/').nth(1).filter(|segment| !segment.is_empty())
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

    let Some(identifier) = script_identifier(parts.uri.path()) else {
        return Ok(text_response(StatusCode::NOT_FOUND, "Invalid script."));
    };
    let identifier = identifier.to_owned();

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

    let mut script_client = state.clients.script.clone();

    let script = script_client
        .query_script(FindScriptRequest {
            query: Some(Query::PublicName(identifier)),
        })
        .await?;

    let Some(current_revision_id) = script.get_ref().current_revision_id.clone() else {
        return Ok(text_response(
            StatusCode::NOT_FOUND,
            "Script did not have a revision.",
        ));
    };

    let revision = script_client
        .get_revision(GetRevisionRequest {
            id: current_revision_id,
            with_bundle: true,
        })
        .await?;

    // Create a context URI without the identifier, used for better routing.
    let old_uri = &parts.uri;
    let path = path::Path::new(old_uri.path());
    let without_identifier: path::PathBuf = path.iter().skip(2).collect();
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

    // The LocalSet dance exists because lua futures are not Send; it is
    // replaced wholesale by the runtime thread pool in P1.3.
    let kv_client = state.clients.kv.clone();
    let script = script.into_inner();
    let revision = revision.into_inner();

    let lua_response: extensions::http::Response = task::block_in_place(move || {
        Handle::current().block_on(async move {
            let local = task::LocalSet::new();
            local
                .run_until(async move {
                    let lua = ActiasRuntime::new(script, revision, kv_client, Some(10)).await?;

                    let listener = lua.listener(ActiasRuntime::FETCH_EVENT)?;

                    lua.start_timer();

                    let value: mlua::Value =
                        listener.call_async(lua.to_value(&lua_request)?).await?;
                    Ok::<_, anyhow::Error>(lua.from_value(value)?)
                })
                .await
        })
    })?;

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

    #[test]
    fn script_identifier_is_the_first_path_segment() {
        assert_eq!(script_identifier("/my-script"), Some("my-script"));
        assert_eq!(script_identifier("/my-script/users/1"), Some("my-script"));
        assert_eq!(script_identifier("/my-script/"), Some("my-script"));
    }

    #[test]
    fn script_identifier_is_absent_when_the_path_names_nothing() {
        // A bare root addresses the worker itself, not a script, so it must not
        // reach the script service with an empty name.
        assert_eq!(script_identifier("/"), None);
        assert_eq!(script_identifier(""), None);
        assert_eq!(script_identifier("//"), None);
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
        let app = router(lazy_clients(), 1024, Duration::from_secs(5));

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
        let app = router(lazy_clients(), 1024, Duration::from_secs(5));

        let request = axum::http::Request::builder()
            .uri("/")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
