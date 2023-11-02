use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use mlua::{ExternalResult, LuaSerdeExt, UserData};
use serde::{Deserialize, Serialize};

use crate::runtime::extension::{ExtensionInfo, LuaExtension};

/// JWT extension.
pub struct JwtExtension;

impl LuaExtension for JwtExtension {
    fn create_extension<'a>(&'a self, lua: &'a mlua::Lua) -> mlua::Result<mlua::Value> {
        let jwt = lua.create_table()?;

        let jwt_class = lua.create_proxy::<JwtClass>()?;
        jwt.set("Jwt", jwt_class.clone())?;
        lua.globals().set("Jwt", jwt_class)?;

        Ok(mlua::Value::Table(jwt))
    }

    fn extension_info(&self) -> ExtensionInfo {
        ExtensionInfo {
            name: "jwt",
            description: "JWT extension for signing/verifying JWTs",
            default: true,
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
struct JwtHeader {
    typ: String,
    alg: Algorithm,
}

struct JwtClass {
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
    header: Header,
}

impl UserData for JwtClass {
    fn add_methods<'lua, M: mlua::UserDataMethods<'lua, Self>>(methods: &mut M) {
        // Static constructor
        methods.add_function("new", |lua, (header, secret): (mlua::Value, String)| {
            let header: JwtHeader = lua.from_value(header)?;
            // Only JWT supported.
            if header.typ != "JWT" {
                return Err(mlua::Error::RuntimeError(format!(
                    "Invalid JWT header type: {}, valid types: [JWT]",
                    header.typ
                )));
            }

            Ok(lua.create_userdata(JwtClass {
                encoding_key: EncodingKey::from_secret(secret.as_bytes()),
                decoding_key: DecodingKey::from_secret(secret.as_bytes()),
                header: Header::new(header.alg),
            })?)
        });

        methods.add_method("encode", |lua, this, payload: mlua::Value| {
            let payload: serde_json::Value = lua.from_value(payload)?;
            let token = encode(&this.header, &payload, &this.encoding_key).into_lua_err()?;

            Ok(token)
        });

        methods.add_method("decode", |lua, this, token: String| {
            Ok(
                match decode::<serde_json::Value>(
                    &token,
                    &this.decoding_key,
                    &Validation::default(),
                ) {
                    Ok(v) => lua.to_value(&v.claims)?,
                    Err(_) => mlua::Value::Nil,
                },
            )
        });
    }
}
