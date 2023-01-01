use std::str::FromStr;

use hyper::header::{self, HeaderName};
use hyper::{Body, Request, Response};
use mlua::LuaSerdeExt;

use crate::runtime::module::LuaModule;
use crate::runtime::request::LuaRequest;
use crate::runtime::{response, EphermalRuntime};

/// Handle lua errors at top level hyper by returning an error response.
macro_rules! lua_check {
    ($res:expr) => {
        match $res {
            Ok(v) => v,
            Err(e) => {
                tracing::trace!(error = e.to_string(), "Error in lua runtime");
                return Ok(Response::new(Body::from({
                    format!("Error in lua runtime: {}", e)
                })));
            }
        }
    };
}

/// Constructs a lua runtime and runs the proper http handler per reuqest.
pub async fn http_handler(request: Request<Body>) -> anyhow::Result<Response<Body>> {
    let lua = lua_check!(EphermalRuntime::new());

    lua_check!(lua.set_module_from_source(LuaModule::new("test.lua", include_str!("test.lua"))));

    // Create a lua userdata request object based on the hyper request.
    let lua_request = LuaRequest::new(request).await;

    // Lua runtime uses registry to store handlers.
    let value = lua_check!(lua.named_registry_value::<str, mlua::Function>("listener_fetch"));
    let ret: response::Response = lua_check!(lua.from_value(lua_check!(value.call(lua_request))));

    // Body returned from response. Defaulted to null if none provided.
    let body = ret.body.unwrap_or(serde_json::Value::Null);

    // Build the response based on the returned json from lua.
    let mut response = Response::builder()
        .status(ret.status_code.unwrap_or(200))
        .body(Body::from(match &body {
            // Small hack so that strings wont have quotes surrounding them.
            serde_json::Value::String(v) => v.to_owned(),
            _ => body.to_string(),
        }))?;

    let headers_mut = response.headers_mut();

    // If the return was a json object or an array we can auto set the `Content-Type` header.
    // User can always manually override this later.
    if let serde_json::Value::Object(_) | serde_json::Value::Array(_) = body {
        headers_mut.insert(header::CONTENT_TYPE, "application/json".parse()?);
    }

    // Copy all headers from lua response.
    if let Some(headers) = ret.headers {
        for (key, value) in headers.iter() {
            headers_mut.insert(
                HeaderName::from_str(key)?,
                value.parse().unwrap_or("".parse()?),
            );
        }
    }

    Ok(response)
}
