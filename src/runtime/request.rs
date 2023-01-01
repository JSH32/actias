use hyper::{HeaderMap, Method, Request, Version};
use mlua::{LuaSerdeExt, UserData, UserDataFields, UserDataMethods};

/// Lua userland request type.
pub struct LuaRequest {
    method: Method,
    headers: HeaderMap,
    body: Body,
    version: Version,
}

impl LuaRequest {
    pub async fn new(request: Request<hyper::Body>) -> Self {
        Self {
            method: request.method().clone(),
            headers: request.headers().clone(),
            version: request.version().clone(),
            body: Body(match hyper::body::to_bytes(request.into_body()).await {
                Ok(v) => v.to_vec(),
                Err(_) => Vec::new(),
            }),
        }
    }
}

impl UserData for LuaRequest {
    fn add_fields<'lua, F: UserDataFields<'lua, Self>>(fields: &mut F) {
        fields.add_field_method_get("body", |_, this| Ok(this.body.clone()));
        fields.add_field_method_get("method", |_, this| Ok(this.method.as_str().to_owned()));
        fields.add_field_method_get("version", |_, this| Ok(format!("{:?}", this.version)));
        fields.add_field_method_get("headers", |lua, this| {
            let headers = lua.create_table()?;

            for (key, value) in this.headers.iter() {
                headers.set(
                    key.as_str(),
                    value
                        .to_str()
                        .map_err(|e| mlua::Error::DeserializeError(e.to_string()))?,
                )?;
            }

            Ok(headers)
        });
    }
}

/// Lua userland transformable body.
#[derive(Clone)]
struct Body(Vec<u8>);

impl UserData for Body {
    fn add_fields<'lua, F: UserDataFields<'lua, Self>>(fields: &mut F) {
        fields.add_field_method_get("is_empty", |_, this| Ok(this.0.is_empty()))
    }

    fn add_methods<'lua, M: UserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_method("json", |lua, this, ()| {
            if this.0.is_empty() {
                Err(mlua::Error::SerializeError("Body is empty.".to_string()))
            } else {
                let json = serde_json::json!(this.0);
                Ok(lua.to_value(&json))
            }
        });

        methods.add_method("text", |lua, this, ()| {
            Ok(lua.to_value(if this.0.is_empty() {
                ""
            } else {
                &std::str::from_utf8(&this.0)?
            }))
        });
    }
}
