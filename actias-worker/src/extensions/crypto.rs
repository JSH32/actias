use argon2::{
    password_hash::{rand_core::OsRng, SaltString},
    Algorithm, Argon2, PasswordHash, PasswordHasher, PasswordVerifier, Version,
};
use mlua::{ExternalResult, IntoLua, UserData};
use rsa::{
    pkcs1::{DecodeRsaPrivateKey, EncodeRsaPrivateKey},
    pkcs8::{DecodePrivateKey, EncodePrivateKey, LineEnding},
    Pkcs1v15Encrypt,
};
use sha2::{Digest, Sha224, Sha256, Sha384, Sha512, Sha512_224, Sha512_256};

use crate::runtime::extension::{ExtensionInfo, LuaExtension};

/// Crypto extension.
/// TODO: Add HMAC, PBKDF
pub struct CryptoExtension;

impl LuaExtension for CryptoExtension {
    fn create_extension<'a>(&'a self, lua: &'a mlua::Lua) -> mlua::Result<mlua::Value<'a>> {
        let crypto = lua.create_table()?;

        let argon2_class = lua.create_proxy::<Argon2Class>()?;
        crypto.set("Argon2", argon2_class.clone())?;

        crypto.set(
            "sha224",
            lua.create_function(|_, input: String| Ok(format!("{:x}", Sha224::digest(input))))?,
        )?;

        crypto.set(
            "sha256",
            lua.create_function(|_, input: String| Ok(format!("{:x}", Sha256::digest(input))))?,
        )?;

        crypto.set(
            "sha512_224",
            lua.create_function(|_, input: String| Ok(format!("{:x}", Sha512_224::digest(input))))?,
        )?;

        crypto.set(
            "sha512_256",
            lua.create_function(|_, input: String| Ok(format!("{:x}", Sha512_256::digest(input))))?,
        )?;

        crypto.set(
            "sha384",
            lua.create_function(|_, input: String| Ok(format!("{:x}", Sha384::digest(input))))?,
        )?;

        crypto.set(
            "sha512",
            lua.create_function(|_, input: String| Ok(format!("{:x}", Sha512::digest(input))))?,
        )?;

        let rsa_private_class = lua.create_proxy::<RsaPrivateKey>()?;
        crypto.set("RsaPrivateKey", rsa_private_class.clone())?;

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

/// RSA private key instance.
struct RsaPrivateKey(rsa::RsaPrivateKey);

impl UserData for RsaPrivateKey {
    fn add_methods<'lua, M: mlua::UserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_function("new", |_, bits: usize| {
            if bits > 4096 {
                return Err(mlua::Error::RuntimeError(
                    "Maximum RSA key size is 4096 bits".to_string(),
                ));
            }

            Ok(RsaPrivateKey(
                rsa::RsaPrivateKey::new(&mut OsRng, bits).unwrap(),
            ))
        });

        methods.add_function("from_pem", |_, (pem, r#type): (String, Option<String>)| {
            Ok(RsaPrivateKey(
                match r#type
                    .or_else(|| Some("PKCS8".to_string()))
                    .unwrap()
                    .as_str()
                {
                    "PKCS8" => rsa::RsaPrivateKey::from_pkcs8_pem(&pem).into_lua_err()?,
                    "PKCS1" => rsa::RsaPrivateKey::from_pkcs1_pem(&pem).into_lua_err()?,
                    _ => {
                        return Err(mlua::Error::RuntimeError(
                            "Unsupported PEM type, supported types: [pkcs8, pkcs1]".to_string(),
                        ))
                    }
                },
            ))
        });

        methods.add_method("to_pem", |_, this, r#type: Option<String>| {
            Ok(match r#type
                .or_else(|| Some("PKCS8".to_string()))
                .unwrap()
                .as_str()
            {
                "PKCS8" => this.0.to_pkcs8_pem(LineEnding::LF).into_lua_err()?,
                "PKCS1" => this.0.to_pkcs1_pem(LineEnding::LF).into_lua_err()?,
                _ => {
                    return Err(mlua::Error::RuntimeError(
                        "Unsupported PEM type, supported types: [pkcs8, pkcs1]".to_string(),
                    ))
                }
            }
            .to_string())
        });

        methods.add_method("public_key", |_, this, _: ()| {
            Ok(RsaPublicKey(rsa::RsaPublicKey::from(&this.0)))
        });

        methods.add_method("decrypt", |lua, this, data: Vec<u8>| {
            Ok(match this.0.decrypt(Pkcs1v15Encrypt, &data) {
                Ok(v) => v.into_lua(lua)?,
                Err(_) => mlua::Value::Nil,
            })
        });
    }
}

/// RSA public key instance.
struct RsaPublicKey(rsa::RsaPublicKey);

impl UserData for RsaPublicKey {
    fn add_methods<'lua, M: mlua::UserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_method("encrypt", |_, this, data: Vec<u8>| {
            Ok(this
                .0
                .encrypt(&mut OsRng, Pkcs1v15Encrypt, &data[..])
                .into_lua_err()?)
        });
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
