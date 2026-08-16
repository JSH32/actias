use crate::proto::kv_service::kv_service_client::KvServiceClient;
use crate::proto::script_service::GetRevisionRequest;
use crate::proto::script_service::find_script_request::Query;

use crate::{ScriptServiceClient, proto::script_service::FindScriptRequest};
use actias_common::tracing::Level;
use actias_common::tracing::{error, span};
use core::result::Result::Ok;
use hyper::Uri;
use hyper::{Body, Request, Response, StatusCode, http};
use mlua::LuaSerdeExt;
use std::path;
use tokio::runtime::Handle;
use tokio::task;

use crate::extensions;
use crate::extensions::http::Request as LuaRequest;
use crate::runtime::ActiasRuntime;

/// Extracts the script identifier from a request path.
///
/// The first path segment selects the script, so `/my-script/users` runs
/// `my-script` and hands it `/users`. A path with no first segment, such as
/// `/`, addresses no script at all.
fn script_identifier(path: &str) -> Option<&str> {
    path.split('/').nth(1).filter(|segment| !segment.is_empty())
}

/// Builds a response whose body is exactly `body`.
fn text_response(status: StatusCode, body: &'static str) -> Response<Body> {
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    response
}

/// Builds the response for a request the runtime could not complete.
///
/// The cause is logged against a correlation id and the client is told only the
/// id, because internal errors quote connection strings, hostnames and paths.
fn internal_error_response(error: &anyhow::Error) -> Response<Body> {
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

/// Constructs a lua runtime and runs the proper http handler per request.
pub async fn http_handler(
    request: Request<Body>,
    script_client: ScriptServiceClient<tonic::transport::Channel>,
    kv_client: KvServiceClient<tonic::transport::Channel>,
) -> anyhow::Result<Response<Body>> {
    let local = task::LocalSet::new();

    let span = span!(Level::DEBUG, "lua_http_request");
    let _enter = span.enter();

    let response = task::block_in_place(move || {
        Handle::current().block_on(async {
            local
                .run_until(async move {
                    let handler = task::spawn_local(lua_handler(request, script_client, kv_client));

                    match handler.await {
                        Ok(Ok(response)) => response,
                        Ok(Err(error)) => internal_error_response(&error),
                        // Panics are not caught inside lua, so reaching here means the
                        // host itself failed rather than the script.
                        Err(join_error) => internal_error_response(&anyhow::Error::new(join_error)),
                    }
                })
                .await
        })
    });

    Ok(response)
}

/// Lua request handler.
async fn lua_handler(
    request: Request<Body>,
    mut script_client: ScriptServiceClient<tonic::transport::Channel>,
    kv_client: KvServiceClient<tonic::transport::Channel>,
) -> anyhow::Result<Response<Body>> {
    let Some(identifier) = script_identifier(request.uri().path()) else {
        return Ok(text_response(StatusCode::NOT_FOUND, "Invalid script."));
    };

    let script = script_client
        .query_script(FindScriptRequest {
            query: Some(Query::PublicName(identifier.to_string())),
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

    let lua = ActiasRuntime::new(
        script.into_inner(),
        revision.into_inner(),
        kv_client,
        Some(10),
    )
    .await?;

    // Create a context URI without the identifier, used for better routing.
    let old_uri = request.uri().clone();
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

    // Copy authority
    if let Some(auth) = old_uri.authority() {
        context_uri = context_uri.authority(auth.clone());
    }

    // Create a lua userdata request object based on the hyper request.
    let lua_request = LuaRequest::new(request, Some(context_uri.build()?)).await;

    let value = lua.listener(ActiasRuntime::FETCH_EVENT)?;

    lua.start_timer();

    let ret: extensions::http::Response =
        lua.from_value(value.call_async(lua.to_value(&lua_request?)?).await?)?;

    // Build the response based on the returned json from lua.
    let response: http::Result<Response<Body>> = ret.into();
    Ok(response?)
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();

        assert!(!body.contains("postgres"), "leaked the cause: {body}");
        assert!(!body.contains("hunter2"), "leaked the cause: {body}");
        assert!(!body.contains("refused"), "leaked the cause: {body}");
        assert!(body.contains("Correlation ID"), "unusable message: {body}");
    }
}
