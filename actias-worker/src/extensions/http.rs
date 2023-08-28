use crate::runtime::extension::{ExtensionInfo, LuaExtension};
use actias_common::tracing::debug;
use hyper::{
    client::HttpConnector,
    header::HeaderName,
    http::{self, uri::InvalidUri},
    Body, Client, Version,
};
use hyper_tls::HttpsConnector;
use mlua::{ExternalResult, LuaSerdeExt, UserData};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, str::FromStr};

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
            lua.create_async_function(|lua, request: mlua::Table| async move {
                // Since we accept userdata, we need to do this hack to allow for conversion.
                let lua_request: Request =
                    serde_json::from_str(&serde_json::to_string(&request).into_lua_err()?)
                        .into_lua_err()?;

                let request: mlua::Result<hyper::Request<_>> = lua_request.clone().into();

                let hyper_uri: http::Result<hyper::Uri> =
                    lua_request.uri.to_uri().into_lua_err()?.into();

                debug!(
                    request = ?lua_request,
                    "Making request to {}", hyper_uri.into_lua_err()?.to_string()
                );

                let client = match lua.app_data_ref::<Client<HttpsConnector<HttpConnector>, Body>>()
                {
                    // Find if exist.
                    Some(v) => v,
                    // Create on first use.
                    None => {
                        lua.set_app_data::<Client<HttpsConnector<HttpConnector>, Body>>(
                            Client::builder().build(HttpsConnector::new()),
                        );

                        lua.app_data_ref::<Client<HttpsConnector<HttpConnector>, Body>>()
                            .unwrap()
                    }
                };

                let response =
                    Response::new(client.request(request?).await.into_lua_err()?).await?;
                Ok(lua.to_value(&response)?)
            })?,
        )?;

        let uri_class = lua.create_proxy::<Uri>()?;
        http.set("Uri", uri_class.clone())?;
        lua.globals().set("Uri", uri_class)?;

        Ok(mlua::Value::Table(http))
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
enum UriType {
    Uri(Uri),
    String(String),
}

impl UriType {
    pub fn to_uri(&self) -> Result<Uri, InvalidUri> {
        Ok(match self {
            UriType::Uri(uri) => uri.clone(),
            UriType::String(v) => Uri::from(hyper::Uri::from_str(&v)?),
        })
    }
}

/// Lua userland request type.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Request {
    uri: UriType,
    /// Only used from server, client making request doesn't need this.
    context_uri: Option<UriType>,
    method: Option<String>,
    #[serde(default)]
    headers: HashMap<String, String>,
    version: Option<String>,
    body: Option<BodyType>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum BodyType {
    Binary(Vec<u8>),
    Text(String),
}

impl BodyType {
    async fn from_hyper(body: hyper::Body) -> mlua::Result<Self> {
        let body_bytes = hyper::body::to_bytes(body).await.into_lua_err()?;

        Ok(match String::from_utf8(body_bytes.to_vec()) {
            Ok(v) => BodyType::Text(v),
            // Not a string
            Err(_) => BodyType::Binary(body_bytes.to_vec()),
        })
    }
}

impl Into<hyper::Body> for BodyType {
    fn into(self) -> hyper::Body {
        match self {
            BodyType::Binary(v) => hyper::Body::from(v),
            BodyType::Text(v) => hyper::Body::from(v),
        }
    }
}

impl Request {
    /// Create a new Lua Request object.
    ///
    /// # Arguments
    ///
    /// * `request` - Hyper request to convert from.
    /// * `context_uri` - URI which is stripped from worker identifier (if relevant).
    pub async fn new(
        request: hyper::Request<hyper::Body>,
        context_uri: Option<hyper::Uri>,
    ) -> mlua::Result<Self> {
        Ok(Self {
            uri: UriType::String(request.uri().to_string()),
            method: Some(request.method().to_string()),
            context_uri: Some(UriType::String(
                context_uri.unwrap_or(request.uri().clone()).to_string(),
            )),
            headers: {
                request
                    .headers()
                    .iter()
                    .map(|h| (h.0.to_string(), h.1.to_str().unwrap_or("").to_string()))
                    .collect()
            },
            version: Some(format!("{:?}", request.version())),
            body: Some(BodyType::from_hyper(request.into_body()).await?),
        })
    }
}

impl UserData for Request {}

impl From<Request> for mlua::Result<hyper::Request<hyper::Body>> {
    fn from(req: Request) -> Self {
        let hyper_uri: http::Result<hyper::Uri> = req.uri.to_uri().into_lua_err()?.into();

        Ok(hyper::Request::builder()
            .uri(hyper_uri.into_lua_err()?)
            .method(req.method.unwrap_or("GET".to_string()).as_str())
            .version(match req.version {
                Some(v) => string_to_version(&v)?,
                None => Version::HTTP_11,
            })
            .body(match req.body {
                Some(v) => v.into(),
                None => hyper::Body::empty(),
            })
            .unwrap())
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Authority {
    pub host: String,
    pub port: Option<u16>,
}

impl UserData for Authority {}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Uri {
    scheme: Option<String>,
    authority: Option<Authority>,
    path: String,
    query: Option<String>,
}

impl UserData for Uri {
    fn add_methods<'lua, M: mlua::UserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_method("tostring", |_, this, ()| {
            let uri: http::Result<hyper::Uri> = this.to_owned().into();
            Ok(uri.into_lua_err()?.to_string())
        });

        // Static constructors
        methods.add_function("new", |lua, uri: mlua::Value| {
            let lua_uri: Uri = lua.from_value(uri)?;
            Ok(lua.create_ser_userdata(lua_uri)?)
        });

        methods.add_function("parse", |lua, uri: String| {
            let uri = hyper::Uri::from_str(&uri).into_lua_err()?;
            Ok(lua.create_ser_userdata(Uri::from(uri))?)
        });
    }

    fn add_fields<'lua, F: mlua::UserDataFields<'lua, Self>>(fields: &mut F) {
        fields.add_field_method_get("scheme", |_, this| Ok(this.scheme.clone()));
        fields.add_field_method_get("authority", |_, this| Ok(this.authority.clone()));
        fields.add_field_method_get("path", |_, this| Ok(this.path.clone()));
        fields.add_field_method_get("query", |_, this| Ok(this.query.clone()));
    }
}

impl From<hyper::Uri> for Uri {
    fn from(uri: hyper::Uri) -> Self {
        Self {
            scheme: uri.scheme_str().map(str::to_string),
            authority: match uri.authority() {
                Some(v) => Some(Authority {
                    host: v.host().to_string(),
                    port: v.port_u16(),
                }),
                None => None,
            },
            path: uri.path().to_string(),
            query: uri.query().map(str::to_string),
        }
    }
}

impl From<Uri> for http::Result<hyper::Uri> {
    fn from(uri: Uri) -> Self {
        let mut builder = hyper::Uri::builder().path_and_query(format!(
            "{}{}",
            uri.path,
            match &uri.query {
                Some(v) => v,
                None => "",
            }
        ));

        if let Some(scheme) = &uri.scheme {
            builder = builder.scheme(scheme.as_str())
        }

        if let Some(authority) = &uri.authority {
            builder = builder.authority(format!(
                "{}{}",
                authority.host,
                match authority.port {
                    Some(v) => format!(":{}", v),
                    None => "".to_string(),
                }
            ));
        }

        builder.build()
    }
}

/// Lua HTTP response. Can be converted to and from lua.
#[derive(Serialize, Deserialize, Clone)]
pub struct Response {
    pub status_code: Option<u16>,
    pub headers: Option<HashMap<String, String>>,
    pub body: Option<BodyType>,
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
            body: Some(
                BodyType::from_hyper(response.into_body())
                    .await
                    .into_lua_err()?,
            ),
        })
    }
}

impl From<Response> for http::Result<hyper::Response<hyper::Body>> {
    fn from(res: Response) -> Self {
        // Build the response based on the returned json from lua.
        let mut response = hyper::Response::builder()
            .status(res.status_code.unwrap_or(200))
            .body(match res.body {
                Some(v) => v.into(),
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
