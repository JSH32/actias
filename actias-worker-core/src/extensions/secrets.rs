//! The `secret "name"` declaration: resolves a project secret through the
//! secret service at declaration time; the handle is the plaintext value.
//!
//! Workers hold no key material. The service decrypts and the value lives
//! only inside the vm that declared it; the request carries the script id
//! as audit metadata (and, once script.json bindings land, the
//! enforcement point).

use mlua::ExternalResult;
use tonic::Code;

use crate::{
    proto::secret_service::{ResolveSecretRequest, secret_service_client::SecretServiceClient},
    runtime::extension::{ExtensionInfo, LuaExtension},
};

/// Secret access for scripts.
pub struct SecretsExtension {
    /// Absent when the worker has no secret service configured, in which
    /// case declaring a secret reports exactly that.
    pub secret_client: Option<SecretServiceClient<tonic::transport::Channel>>,
    pub project_id: String,
    /// Script whose vms declare; audit metadata on every resolution.
    pub script_id: String,
}

impl LuaExtension for SecretsExtension {
    fn extension_info(&self) -> ExtensionInfo<'_> {
        ExtensionInfo {
            name: "secret",
            description: "Project secrets, resolved per declaration",
            default: true,
        }
    }

    fn create_extension(&self, lua: &mlua::Lua) -> mlua::Result<mlua::Value> {
        let secret_client = self.secret_client.clone();
        let project_id = self.project_id.clone();
        let script_id = self.script_id.clone();

        // `local token = secret "name"`: the handle is the string value
        // itself, resolved at declaration.
        let declaration = lua.create_async_function(move |lua, name: String| {
            let secret_client = secret_client.clone();
            let project_id = project_id.clone();
            let script_id = script_id.clone();

            async move {
                crate::runtime::ActiasRuntime::assert_declaration_phase(&lua, "secret")?;
                crate::runtime::ActiasRuntime::assert_contract_allows(
                    &lua,
                    crate::runtime::ContractKind::Secret,
                    &name,
                )?;
                crate::runtime::ActiasRuntime::record_secret_declaration(&lua, &name);

                let Some(mut secret_client) = secret_client else {
                    return Err(mlua::Error::RuntimeError(
                        "Secrets are not configured on this worker.".to_owned(),
                    ));
                };

                let resolved = secret_client
                    .resolve_secret(ResolveSecretRequest {
                        project_id,
                        name: name.clone(),
                        version: 0,
                        script_id,
                    })
                    .await;

                match resolved {
                    Ok(resolved) => Ok(resolved.into_inner().value),
                    Err(status) if status.code() == Code::NotFound => {
                        Err(mlua::Error::RuntimeError(format!(
                            "Secret '{name}' is not set for this project."
                        )))
                    }
                    Err(status) => Err(status).into_lua_err(),
                }
            }
        })?;

        Ok(mlua::Value::Function(declaration))
    }
}
