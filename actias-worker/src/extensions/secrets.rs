//! The `secret "name"` declaration: fetches a project secret from the
//! reserved kv namespace and hands the script its plaintext.
//!
//! The api stores values as `base64(nonce || ciphertext || tag)` under
//! AES-256-GCM (see [`actias_common::naming::SECRETS_NAMESPACE`]); this side
//! decrypts with the shared `SECRET_ENCRYPTION_KEY`.

use aes_gcm::{Aes256Gcm, KeyInit, aead::Aead};
use base64::Engine;
use mlua::ExternalResult;
use std::sync::Arc;
use tonic::Code;

use crate::{
    proto::kv_service::{PairRequest, kv_service_client::KvServiceClient},
    runtime::extension::{ExtensionInfo, LuaExtension},
};

/// Bytes of the AES-256-GCM key.
pub const KEY_LEN: usize = 32;
/// Bytes of the GCM nonce prefixed to every stored value.
const NONCE_LEN: usize = 12;

/// Secret access for scripts.
pub struct SecretsExtension {
    pub kv_client: KvServiceClient<tonic::transport::Channel>,
    pub project_id: String,
    /// Absent when the worker has no `SECRET_ENCRYPTION_KEY`, in which case
    /// declaring a secret reports exactly that.
    pub key: Option<Arc<[u8; KEY_LEN]>>,
}

/// Decrypts one stored secret value.
///
/// # Errors
/// Returns [`mlua::Error::RuntimeError`] when the value is not the expected
/// shape or fails authentication; the message never includes the payload.
fn decrypt(key: &[u8; KEY_LEN], stored: &str) -> mlua::Result<String> {
    let corrupt = || mlua::Error::RuntimeError("Stored secret could not be decrypted.".to_owned());

    let data = base64::engine::general_purpose::STANDARD
        .decode(stored)
        .map_err(|_| corrupt())?;

    if data.len() <= NONCE_LEN {
        return Err(corrupt());
    }

    let nonce: [u8; NONCE_LEN] = data[..NONCE_LEN].try_into().map_err(|_| corrupt())?;

    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| corrupt())?;
    let plaintext = cipher
        .decrypt(&nonce.into(), &data[NONCE_LEN..])
        .map_err(|_| corrupt())?;

    String::from_utf8(plaintext).map_err(|_| corrupt())
}

impl LuaExtension for SecretsExtension {
    fn extension_info(&self) -> ExtensionInfo<'_> {
        ExtensionInfo {
            name: "secret",
            description: "Project secrets, decrypted per declaration",
            default: true,
        }
    }

    fn create_extension(&self, lua: &mlua::Lua) -> mlua::Result<mlua::Value> {
        let kv_client = self.kv_client.clone();
        let project_id = self.project_id.clone();
        let key = self.key.clone();

        // `local token = secret "name"`: the handle is the string value
        // itself (docs/SURFACE.md), fetched and decrypted at declaration.
        let declaration = lua.create_async_function(move |lua, name: String| {
            let mut kv_client = kv_client.clone();
            let project_id = project_id.clone();
            let key = key.clone();

            async move {
                crate::runtime::ActiasRuntime::assert_declaration_phase(&lua, "secret")?;
                crate::runtime::ActiasRuntime::record_secret_declaration(&lua, &name);

                let Some(key) = key else {
                    return Err(mlua::Error::RuntimeError(
                        "Secrets are not configured on this worker.".to_owned(),
                    ));
                };

                let pair = kv_client
                    .get_pair(PairRequest {
                        project_id,
                        namespace: actias_common::naming::SECRETS_NAMESPACE.to_owned(),
                        key: name.clone(),
                    })
                    .await;

                let stored = match pair {
                    Ok(pair) => pair.into_inner().value,
                    Err(status) if status.code() == Code::NotFound => {
                        return Err(mlua::Error::RuntimeError(format!(
                            "Secret '{name}' is not set for this project."
                        )));
                    }
                    Err(status) => return Err(status).into_lua_err(),
                };

                decrypt(&key, &stored)
            }
        })?;

        Ok(mlua::Value::Function(declaration))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cross-language vector: produced by the api's node implementation
    /// (`crypto.createCipheriv("aes-256-gcm", ...)`) with the key below. If
    /// either side changes its format, this fails rather than production.
    #[test]
    fn decrypts_a_value_the_api_encrypted() {
        let key: [u8; KEY_LEN] = *b"0123456789abcdef0123456789abcdef";
        let stored = "AAAAAAAAAAAAAAAApgbPdXdxbZBdwEHhIl2ku94XU2VO5Mw=";

        assert_eq!(decrypt(&key, stored).unwrap(), "hunter2");
    }

    #[test]
    fn a_tampered_value_reports_corruption_without_detail() {
        let key: [u8; KEY_LEN] = *b"0123456789abcdef0123456789abcdef";

        for stored in [
            "not base64 !!!",
            "AAAA",
            "AAAAAAAAAAAAAAAAqgbPdXdxbZBdwEHhIl2ku94XU2VO5Mw=",
        ] {
            let error = decrypt(&key, stored).expect_err("must fail");
            assert!(
                error.to_string().contains("could not be decrypted"),
                "wrong error for {stored:?}: {error}"
            );
        }
    }
}
