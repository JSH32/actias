use crate::proto::script_service::find_script_request::Query;
use crate::proto::script_service::GetRevisionRequest;

use crate::{proto::script_service::FindScriptRequest, ScriptServiceClient};
use core::result::Result::Ok;
use ephermal_common::tracing::Level;
use ephermal_common::tracing::{span, trace};
use hyper::Uri;
use hyper::{http, Body, Request, Response};
use mlua::LuaSerdeExt;
use std::path;
use tokio::runtime::Handle;
use tokio::task;

use crate::extensions;
use crate::extensions::http::Request as LuaRequest;
use crate::runtime::EphermalRuntime;

/// Constructs a lua runtime and runs the proper http handler per reuqest.
pub async fn http_handler(
    request: Request<Body>,
    script_client: ScriptServiceClient<tonic::transport::Channel>,
) -> anyhow::Result<Response<Body>> {
    let local = task::LocalSet::new();

    let span = span!(Level::DEBUG, "lua_http_request");
    let _enter = span.enter();

    task::block_in_place(move || {
        Handle::current().block_on(async {
            local
                .run_until(async move {
                    task::spawn_local(async move {
                        match lua_handler(request, script_client).await {
                            Ok(v) => Ok(v),
                            Err(e) => {
                                trace!(error = e.to_string(), "Error handling request");
                                Ok(Response::builder().status(500).body(Body::from(format!(
                                    "Error handling request: {}",
                                    e.to_string()
                                )))?)
                            }
                        }
                    })
                    .await
                    .unwrap()
                })
                .await
        })
    })
}

/// Lua request handler.
async fn lua_handler(
    request: Request<Body>,
    mut script_client: ScriptServiceClient<tonic::transport::Channel>,
) -> anyhow::Result<Response<Body>> {
    let path_split: Vec<&str> = request.uri().path().split('/').collect();
    let identifier = path_split.get(1);

    // No identifier
    let revision = if let Some(identifier) = identifier {
        let script = script_client
            .query_script(FindScriptRequest {
                query: Some(Query::PublicName(identifier.to_string())),
            })
            .await?;

        let current_revision_id = script.get_ref().current_revision_id.clone();
        if current_revision_id.is_none() {
            return Ok(Response::builder()
                .status(404)
                .body(Body::from("Script did not have a revision."))
                .unwrap());
        }

        script_client
            .get_revision(GetRevisionRequest {
                id: current_revision_id.unwrap(),
                with_bundle: true,
            })
            .await?
    } else {
        return Ok(Response::builder()
            .status(404)
            .body(Body::from("Invalid script"))
            .unwrap());
    };

    let lua =
        EphermalRuntime::new(identifier.map(|s| s.to_string()), revision.into_inner()).await?;

    // Create a context URI without the identifier, used for better routing.
    let old_uri = request.uri().clone();
    let path = path::Path::new(old_uri.path());
    let without_identifier: path::PathBuf = path.iter().skip(2).collect();
    let mut context_uri = Uri::builder().path_and_query(format!(
        "/{}{}",
        without_identifier.as_path().to_str().unwrap_or(""),
        old_uri.query().unwrap_or("")
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

    // Lua runtime uses registry to store handlers.
    let value = lua.named_registry_value::<str, mlua::Function>("listener_fetch")?;
    let ret: extensions::http::Response =
        lua.from_value(value.call_async(lua.to_value(&lua_request?)?).await?)?;

    // Build the response based on the returned json from lua.
    let response: http::Result<Response<Body>> = ret.into();
    Ok(response?)
}
