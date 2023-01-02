use hyper::{http, Body, Request, Response};
use mlua::LuaSerdeExt;
use tokio::runtime::Handle;
use tokio::task;
use tracing::span;
use tracing::Level;

use crate::extensions;
use crate::extensions::http::Request as LuaRequest;
use crate::runtime::EphermalRuntime;
use crate::runtime::LuaModule;

/// Constructs a lua runtime and runs the proper http handler per reuqest.
pub async fn http_handler(request: Request<Body>) -> anyhow::Result<Response<Body>> {
    let local = task::LocalSet::new();

    let span = span!(Level::TRACE, "lua_http_request");
    let _enter = span.enter();

    task::block_in_place(move || {
        Handle::current().block_on(async {
            local
                .run_until(async move {
                    task::spawn_local(async move {
                        match lua_handler(request).await {
                            Ok(v) => Ok(v),
                            Err(e) => {
                                tracing::trace!(error = e.to_string(), "Error handling request");
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
async fn lua_handler(request: Request<Body>) -> anyhow::Result<Response<Body>> {
    let lua = EphermalRuntime::new()?;

    lua.set_module_from_source(LuaModule::new("test.lua", include_str!("test.lua")))
        .await?;

    // Create a lua userdata request object based on the hyper request.
    let lua_request = LuaRequest::new(request).await;

    // Lua runtime uses registry to store handlers.
    let value = lua.named_registry_value::<str, mlua::Function>("listener_fetch")?;
    let ret: extensions::http::Response = lua.from_value(value.call_async(lua_request).await?)?;

    // Build the response based on the returned json from lua.
    let response: http::Result<Response<Body>> = ret.into();
    Ok(response?)
}
