use argon2::{
    password_hash::{rand_core::OsRng, SaltString},
    Algorithm, Argon2, PasswordHash, PasswordHasher, PasswordVerifier, Version,
};
use mlua::UserData;

use crate::runtime::extension::{ExtensionInfo, LuaExtension};

/// Crypto extension.
pub struct CryptoExtension;

impl LuaExtension for CryptoExtension {
    fn create_extension<'a>(&'a self, lua: &'a mlua::Lua) -> mlua::Result<mlua::Value> {
        let crypto = lua.create_table()?;

        let argon2_class = lua.create_proxy::<Argon2Class>()?;
        crypto.set("Argon2", argon2_class.clone())?;

        Ok(mlua::Value::Table(crypto))
    }

    fn extension_info(&self) -> crate::runtime::extension::ExtensionInfo {
        ExtensionInfo {
            name: "crypto",
            description: "Crypto extension containing various cryptograpic algorithms",
            default: true,
        }
    }
}

/// Argon2 class.
struct Argon2Class(Argon2<'static>);

impl UserData for Argon2Class {
    fn add_methods<'lua, M: mlua::UserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_function("new", move |lua, algorithm: String| {
            let algorithm = match algorithm.as_str() {
                "Argon2i" => Algorithm::Argon2i,
                "Argon2d" => Algorithm::Argon2d,
                "Argon2id" => Algorithm::Argon2id,
                _ => {
                    return Err(mlua::Error::RuntimeError(format!(
                        "Unsupported algorithm: {}, , valid types: [Argon2i, Argon2d, Argon2id]",
                        algorithm
                    )))
                }
            };

            Ok(lua.create_userdata(Self(Argon2::new(
                algorithm,
                Version::V0x13,
                argon2::Params::default(),
            )))?)
        });

        methods.add_method("hash", |_, this, password: String| {
            let salt = SaltString::generate(&mut OsRng);

            let hash = this
                .0
                .hash_password(password.as_bytes(), &salt)
                .map_err(|e| mlua::Error::RuntimeError(e.to_string()))?;

            Ok(hash.to_string())
        });

        methods.add_method("verify", |_, this, (hash, password): (String, String)| {
            let hash =
                PasswordHash::new(&hash).map_err(|e| mlua::Error::RuntimeError(e.to_string()))?;

            Ok(this
                .0
                .verify_password(password.as_bytes(), &hash)
                .map_err(|e| mlua::Error::RuntimeError(e.to_string()))
                .is_ok())
        });
    }
}
