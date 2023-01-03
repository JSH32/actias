use std::fs;
use std::io::Write;

use ephermal_common::models::Bundle;
use ephermal_common::models::File;
use hyper::{http, Body, Request, Response};
use mlua::LuaSerdeExt;
use tokio::runtime::Handle;
use tokio::task;
use tracing::span;
use tracing::Level;
use walkdir::WalkDir;

use crate::extensions;
use crate::extensions::http::Request as LuaRequest;
use crate::runtime::EphermalRuntime;

/// Constructs a lua runtime and runs the proper http handler per reuqest.
pub async fn http_handler(request: Request<Body>) -> anyhow::Result<Response<Body>> {
    let local = task::LocalSet::new();

    let span = span!(Level::DEBUG, "lua_http_request");
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
    let lua = EphermalRuntime::new(Some(read_to_bundle())).await?;

    // Create a lua userdata request object based on the hyper request.
    let lua_request = LuaRequest::new(request).await;

    // Lua runtime uses registry to store handlers.
    let value = lua.named_registry_value::<str, mlua::Function>("listener_fetch")?;
    let ret: extensions::http::Response =
        lua.from_value(value.call_async(lua.to_value(&lua_request?)?).await?)?;

    // Build the response based on the returned json from lua.
    let response: http::Result<Response<Body>> = ret.into();
    Ok(response?)
}

pub fn read_to_bundle() -> Bundle {
    let mut files: Vec<File> = Vec::new();

    for e in WalkDir::new("../test").into_iter().filter_map(|e| e.ok()) {
        if e.metadata().unwrap().is_file() {
            let display_name = e.path().strip_prefix("../test").unwrap();

            files.push(File {
                file_name: e
                    .path()
                    .file_name()
                    .to_owned()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_string(),
                path: display_name.to_str().unwrap().to_string(),
                content: fs::read(e.path().to_str().unwrap()).unwrap(),
            });
        }
    }

    let bundle = Bundle {
        entry_point: "test.lua".into(),
        files,
    };

    let mut file = std::fs::File::create("bundle.json").unwrap();
    file.write_all(serde_json::to_string(&bundle).unwrap().as_bytes())
        .unwrap();

    bundle
}
