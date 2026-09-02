//! The `jwt` surface: signing and verifying tokens with the algorithm
//! the caller names.

use std::str::FromStr;

use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use mlua::{ExternalResult, LuaSerdeExt, UserData};

use crate::runtime::extension::{ExtensionInfo, LuaExtension};

pub struct JwtExtension;

impl LuaExtension for JwtExtension {
    fn create_extension(&self, lua: &mlua::Lua) -> mlua::Result<mlua::Value> {
        let jwt = lua.create_table()?;

        let jwt_class = lua.create_proxy::<JwtClass>()?;
        jwt.set("Jwt", jwt_class.clone())?;
        lua.globals().set("Jwt", jwt_class)?;

        Ok(mlua::Value::Table(jwt))
    }

    fn extension_info(&self) -> ExtensionInfo<'_> {
        ExtensionInfo {
            name: "jwt",
            description: "JWT extension for signing/verifying JWTs",
            default: true,
        }
    }
}

struct JwtClass {
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
    header: Header,
}

impl UserData for JwtClass {
    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        // Static constructor
        methods.add_function("new", |lua, (algorithm, secret): (String, String)| {
            let algorithm: Algorithm = Algorithm::from_str(&algorithm).map_err(|e| {
                mlua::Error::runtime(format!("Invalid JWT Algorithm provided: {}", e))
            })?;
            // The key is a shared secret, so only the HMAC family can
            // sign or verify with it.
            if !matches!(
                algorithm,
                Algorithm::HS256 | Algorithm::HS384 | Algorithm::HS512
            ) {
                return Err(mlua::Error::runtime(
                    "Only HS256, HS384 and HS512 are offered; the key is a shared secret.",
                ));
            }

            lua.create_userdata(JwtClass {
                encoding_key: EncodingKey::from_secret(secret.as_ref()),
                decoding_key: DecodingKey::from_secret(secret.as_ref()),
                header: Header::new(algorithm),
            })
        });

        methods.add_method("encode", |lua, this, payload: mlua::Value| {
            let payload: serde_json::Value = lua.from_value(payload)?;
            let token = encode::<serde_json::Value>(&this.header, &payload, &this.encoding_key)
                .into_lua_err()?;

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
