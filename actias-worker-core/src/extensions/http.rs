//! Outbound http for scripts. Every request and every redirect hop
//! passes the egress policy, so a script cannot reach the cluster's own
//! network by following a redirect it did not write.

use crate::egress::EgressClient;
use crate::runtime::extension::{ExtensionInfo, LuaExtension};
use actias_common::tracing::debug;
use http::uri::InvalidUri;
use mlua::{ExternalResult, LuaSerdeExt, UserData};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, str::FromStr};

/// Http operations.
pub struct HttpExtension {
    /// Shared client whose resolver and redirect handling enforce the
    /// platform's egress policy.
    pub egress: EgressClient,
}

impl LuaExtension for HttpExtension {
    fn extension_info(&self) -> ExtensionInfo<'_> {
        ExtensionInfo {
            name: "http",
            description: "HTTP operations",
            default: true,
        }
    }

    fn create_extension(&self, lua: &mlua::Lua) -> mlua::Result<mlua::Value> {
        let http = lua.create_table()?;
        let egress = self.egress.clone();

        http.set(
            "make_request",
            lua.create_async_function(move |lua, request: mlua::Table| {
                let egress = egress.clone();
                async move {
                    // Since we accept userdata, we need to do this hack to allow for conversion.
                    let lua_request: Request =
                        serde_json::from_str(&serde_json::to_string(&request).into_lua_err()?)
                            .into_lua_err()?;

                    debug!(request = ?lua_request, "Making outbound request");

                    // A request that deferred its object writes settles
                    // them before anything it says leaves the machine.
                    let gates = lua
                        .app_data_ref::<std::sync::Arc<crate::objects::PendingGates>>()
                        .map(|gates| gates.clone());
                    if let Some(gates) = gates {
                        gates.settle().await.map_err(mlua::Error::runtime)?;
                    }

                    // The project's egress lists ride the vm as app data;
                    // a vm nobody gave a policy runs on the node's alone.
                    let scope = lua
                        .app_data_ref::<crate::egress::ScopeEgress>()
                        .map(|scope| scope.clone());
                    let response = lua_request.send(&egress, scope.as_ref()).await?;
                    lua.to_value(&response)
                }
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
            UriType::String(v) => Uri::from(http::Uri::from_str(v)?),
        })
    }
}

/// Lua userland request type.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Request {
    uri: UriType,
    /// Only used from server, client making request doesn't need this.
    context_uri: Option<UriType>,
    /// Server requests only: the path relative to the script (route
    /// segments the platform consumed are stripped), what handlers
    /// route on without parsing uris themselves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    /// Server requests only: decoded query parameters, last value per
    /// key; always a table when the request came off the wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    query: Option<HashMap<String, String>>,
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
    /// Wraps raw bytes, as text when they are valid utf-8.
    fn from_bytes(bytes: Vec<u8>) -> Self {
        match String::from_utf8(bytes) {
            Ok(v) => BodyType::Text(v),
            Err(e) => BodyType::Binary(e.into_bytes()),
        }
    }

    /// Converts into the body type the server's wire response uses.
    pub fn into_axum_body(self) -> axum::body::Body {
        match self {
            BodyType::Binary(v) => axum::body::Body::from(v),
            BodyType::Text(v) => axum::body::Body::from(v),
        }
    }
}

impl From<BodyType> for reqwest::Body {
    fn from(val: BodyType) -> Self {
        match val {
            BodyType::Binary(v) => reqwest::Body::from(v),
            BodyType::Text(v) => reqwest::Body::from(v),
        }
    }
}

impl Request {
    /// Builds the request table a script's fetch listener receives.
    ///
    /// Takes plain values rather than a server request type, so the surface
    /// works the same whatever http stack the server runs.
    ///
    /// # Arguments
    /// * `method` - Http method name.
    /// * `uri` - The uri as requested.
    /// * `context_uri` - The uri with the worker identifier stripped, for
    ///   routing inside the script; falls back to `uri`.
    /// * `headers` - Header names to values.
    /// * `version` - Http version, in `HTTP/1.1` form.
    /// * `body` - Raw body bytes; exposed to lua as text when valid utf-8.
    pub fn from_parts(
        method: String,
        uri: String,
        context_uri: Option<String>,
        headers: HashMap<String, String>,
        version: String,
        body: Vec<u8>,
    ) -> Self {
        let context = context_uri.unwrap_or_else(|| uri.clone());
        let (path, query) = match http::Uri::from_str(&context) {
            Ok(parsed) => (
                Some(parsed.path().to_string()),
                Some(
                    parsed
                        .query()
                        .map(|raw| {
                            url::form_urlencoded::parse(raw.as_bytes())
                                .into_owned()
                                .collect::<HashMap<String, String>>()
                        })
                        .unwrap_or_default(),
                ),
            ),
            Err(_) => (None, None),
        };
        Self {
            context_uri: Some(UriType::String(context)),
            uri: UriType::String(uri),
            path,
            query,
            method: Some(method),
            headers,
            version: Some(version),
            body: Some(BodyType::from_bytes(body)),
        }
    }

    /// Sends the request through the guarded client.
    ///
    /// The url is checked before anything is sent; this is the layer that
    /// catches literal ip destinations, which never reach the client's dns
    /// resolver. Hostnames are checked again at resolution time.
    async fn send(
        self,
        egress: &EgressClient,
        scope: Option<&crate::egress::ScopeEgress>,
    ) -> mlua::Result<Response> {
        let uri_string = {
            let uri: http::Result<http::Uri> = self.uri.to_uri().into_lua_err()?.into();
            uri.into_lua_err()?.to_string()
        };

        let url = url::Url::parse(&uri_string).into_lua_err()?;
        egress.policy.check_url(&url, scope).into_lua_err()?;

        let method =
            reqwest::Method::from_str(self.method.as_deref().unwrap_or("GET")).into_lua_err()?;

        let mut builder = egress.client.request(method, url);

        for (key, value) in self.headers {
            builder = builder.header(key, value);
        }

        if let Some(version) = &self.version {
            builder = builder.version(string_to_version(version)?);
        }

        if let Some(body) = self.body {
            builder = builder.body(reqwest::Body::from(body));
        }

        Response::new(builder.send().await.into_lua_err()?).await
    }
}

impl UserData for Request {}

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
    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("tostring", |_, this, ()| {
            let uri: http::Result<http::Uri> = this.to_owned().into();
            Ok(uri.into_lua_err()?.to_string())
        });

        // Static constructors
        methods.add_function("new", |lua, uri: mlua::Value| {
            let lua_uri: Uri = lua.from_value(uri)?;
            lua.create_ser_userdata(lua_uri)
        });

        methods.add_function("parse", |lua, uri: String| {
            let uri = http::Uri::from_str(&uri).into_lua_err()?;
            lua.create_ser_userdata(Uri::from(uri))
        });
    }

    fn add_fields<F: mlua::UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("scheme", |_, this| Ok(this.scheme.clone()));
        fields.add_field_method_get("authority", |_, this| Ok(this.authority.clone()));
        fields.add_field_method_get("path", |_, this| Ok(this.path.clone()));
        fields.add_field_method_get("query", |_, this| Ok(this.query.clone()));
    }
}

impl From<http::Uri> for Uri {
    fn from(uri: http::Uri) -> Self {
        Self {
            scheme: uri.scheme_str().map(str::to_string),
            authority: uri.authority().map(|v| Authority {
                host: v.host().to_string(),
                port: v.port_u16(),
            }),
            path: uri.path().to_string(),
            query: uri.query().map(str::to_string),
        }
    }
}

impl From<Uri> for http::Result<http::Uri> {
    fn from(uri: Uri) -> Self {
        let mut builder = http::Uri::builder().path_and_query(format!(
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
    pub async fn new(response: reqwest::Response) -> mlua::Result<Self> {
        Ok(Self {
            status_code: Some(response.status().as_u16()),
            headers: Some(
                response
                    .headers()
                    .iter()
                    .map(|h| (h.0.to_string(), h.1.to_str().unwrap_or("").to_string()))
                    .collect(),
            ),
            body: Some(BodyType::from_bytes(
                response.bytes().await.into_lua_err()?.to_vec(),
            )),
        })
    }
}

fn string_to_version(str: &str) -> mlua::Result<http::Version> {
    Ok(match str {
        "HTTP/0.9" => http::Version::HTTP_09,
        "HTTP/1.0" => http::Version::HTTP_10,
        "HTTP/1.1" => http::Version::HTTP_11,
        "HTTP/2.0" => http::Version::HTTP_2,
        "HTTP/3.0" => http::Version::HTTP_3,
        _ => {
            return Err(mlua::Error::DeserializeError(format!(
                "'{}' was not a valid HTTP version.",
                str
            )));
        }
    })
}

#[cfg(test)]
mod tests {
    use crate::egress::{EgressClient, EgressPolicy};
    use crate::proto::bundle::Bundle;
    use crate::proto::kv_service::kv_service_client::KvServiceClient;
    use crate::proto::script_service::{Revision, Script};
    use crate::runtime::{ActiasRuntime, PreparedRevision};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    /// A full runtime over an empty bundle, unconnectable kv, and `policy`
    /// guarding its outbound http.
    async fn runtime_guarded_by(policy: EgressPolicy) -> ActiasRuntime {
        let channel = tonic::transport::Channel::from_static("http://127.0.0.1:1").connect_lazy();

        let prepared = PreparedRevision::prepare(
            Script::default(),
            Revision {
                bundle: Some(Bundle {
                    entry_point: "main.lua".to_owned(),
                    files: vec![],
                }),
                ..Default::default()
            },
        )
        .expect("empty revision prepares");

        ActiasRuntime::new(
            Arc::new(prepared),
            KvServiceClient::new(crate::plain_grpc(channel)),
            EgressClient::new(policy).expect("client builds"),
            None,
            None,
            None,
        )
        .await
        .expect("runtime builds")
    }

    /// Runs `http.make_request` against `uri` and returns the result.
    async fn make_request(lua: &ActiasRuntime, uri: &str) -> mlua::Result<mlua::Value> {
        lua.load(format!("return http.make_request({{ uri = \"{uri}\" }})"))
            .eval_async()
            .await
    }

    /// Listener that flips `hit` if anything ever connects to it.
    async fn tripwire() -> (std::net::SocketAddr, Arc<AtomicBool>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hit = Arc::new(AtomicBool::new(false));

        let flag = hit.clone();
        tokio::spawn(async move {
            if listener.accept().await.is_ok() {
                flag.store(true, Ordering::SeqCst);
            }
        });

        (addr, hit)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_literal_private_ip_is_denied_before_any_connection() {
        let (addr, hit) = tripwire().await;
        let lua = runtime_guarded_by(EgressPolicy::new([], false)).await;

        let result = make_request(&lua, &format!("http://{addr}/")).await;

        let error = result.expect_err("a private literal ip must be denied");
        assert!(
            error.to_string().contains("outbound request denied"),
            "wrong error: {error}"
        );
        assert!(
            !hit.load(Ordering::SeqCst),
            "the request reached the socket"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_hostname_resolving_to_a_private_ip_is_denied_at_resolution() {
        // localhost passes the literal-ip check because it is a name; only
        // the resolver layer can stop it, which is what this proves.
        let (addr, hit) = tripwire().await;
        let lua = runtime_guarded_by(EgressPolicy::new([], false)).await;

        let result = make_request(&lua, &format!("http://localhost:{}/", addr.port())).await;

        assert!(result.is_err(), "localhost must be denied at dns time");
        assert!(
            !hit.load(Ordering::SeqCst),
            "the request reached the socket"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_denied_service_name_is_rejected_by_name() {
        let lua = runtime_guarded_by(EgressPolicy::new(["kv_service".to_owned()], false)).await;

        let error = make_request(&lua, "http://kv_service:50051/")
            .await
            .expect_err("a denied service name must be rejected");

        assert!(
            error.to_string().contains("outbound request denied"),
            "wrong error: {error}"
        );
    }
}
