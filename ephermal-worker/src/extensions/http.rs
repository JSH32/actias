use crate::runtime::extension::{ExtensionInfo, LuaExtension};
use hyper::{header::HeaderName, http, Body, Client, Version};
use hyper_tls::HttpsConnector;
use mlua::{ExternalResult, LuaSerdeExt, UserData};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, str::FromStr};
use tracing::debug;

/// Http oerations.
pub struct HttpExtension;

impl LuaExtension for HttpExtension {
    fn extension_info(&self) -> ExtensionInfo {
        ExtensionInfo {
            name: "http",
            description: "HTTP operations",
            default: true,
        }
    }

    fn create_extension<'a>(&'a self, lua: &'a mlua::Lua) -> mlua::Result<mlua::Value> {
        // Http request method
        let http = lua.create_table()?;
        http.set(
            "make_request",
            lua.create_async_function(|lua, request: mlua::Value| async move {
                let lua_request: Request = lua.from_value(request)?;
                let request: mlua::Result<hyper::Request<_>> = lua_request.clone().into();

                debug!(
                    request = ?lua_request,
                    "Making request to {}", lua_request.uri
                );

                let response = Response::new(
                    Client::builder()
                        .build(HttpsConnector::new())
                        .request(request?)
                        .await
                        .to_lua_err()?,
                )
                .await?;

                Ok(lua.to_value(&response)?)
            })?,
        )?;

        Ok(mlua::Value::Table(http))
    }
}

/// Lua userland request type.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Request {
    uri: String,
    method: String,
    #[serde(default)]
    headers: HashMap<String, String>,
    version: Option<String>,
    body: Option<String>,
}

impl Request {
    pub async fn new(request: hyper::Request<Body>) -> mlua::Result<Self> {
        Ok(Self {
            uri: request.uri().to_string(),
            method: request.method().to_string(),
            headers: {
                request
                    .headers()
                    .iter()
                    .map(|h| (h.0.to_string(), h.1.to_str().unwrap_or("").to_string()))
                    .collect()
            },
            version: Some(format!("{:?}", request.version())),
            body: Some(body_to_string(request.into_body()).await?),
        })
    }
}

impl UserData for Request {}

impl From<Request> for mlua::Result<hyper::Request<Body>> {
    fn from(req: Request) -> Self {
        Ok(hyper::Request::builder()
            .uri(req.uri)
            .method(req.method.as_str())
            .version(match req.version {
                Some(v) => string_to_version(&v)?,
                None => Version::HTTP_11,
            })
            .body(match req.body {
                Some(v) => Body::from(v),
                None => Body::empty(),
            })
            .unwrap())
    }
}

/// Lua HTTP response. Can be converted to and from lua.
#[derive(Serialize, Deserialize, Clone)]
pub struct Response {
    pub status_code: Option<u16>,
    pub headers: Option<HashMap<String, String>>,
    pub body: Option<String>,
}

impl UserData for Response {}

impl Response {
    pub async fn new(response: hyper::Response<hyper::Body>) -> mlua::Result<Self> {
        Ok(Self {
            status_code: Some(response.status().as_u16()),
            headers: Some(
                response
                    .headers()
                    .iter()
                    .map(|h| (h.0.to_string(), h.1.to_str().unwrap_or("").to_string()))
                    .collect(),
            ),
            body: Some(body_to_string(response.into_body()).await?),
        })
    }
}

impl From<Response> for http::Result<hyper::Response<Body>> {
    fn from(res: Response) -> Self {
        // Build the response based on the returned json from lua.
        let mut response = hyper::Response::builder()
            .status(res.status_code.unwrap_or(200))
            .body(match res.body {
                Some(v) => hyper::Body::from(v),
                None => hyper::Body::empty(),
            })?;

        let headers_mut = response.headers_mut();

        // Copy all headers from lua response.
        if let Some(headers) = res.headers {
            for (key, value) in headers.iter() {
                headers_mut.insert(
                    HeaderName::from_str(key)?,
                    value.parse().unwrap_or("".parse()?),
                );
            }
        }

        Ok(response)
    }
}

fn string_to_version(str: &str) -> mlua::Result<Version> {
    Ok(match str {
        "HTTP/0.9" => Version::HTTP_09,
        "HTTP/1.0" => Version::HTTP_10,
        "HTTP/1.1" => Version::HTTP_11,
        "HTTP/2.0" => Version::HTTP_2,
        "HTTP/3.0" => Version::HTTP_3,
        _ => {
            return Err(mlua::Error::DeserializeError(format!(
                "'{}' was not a valid HTTP version.",
                str
            )))
        }
    })
}

async fn body_to_string(body: hyper::Body) -> mlua::Result<String> {
    let body_bytes = hyper::body::to_bytes(body).await.to_lua_err()?;
    Ok(String::from_utf8(body_bytes.to_vec()).to_lua_err()?)
}
