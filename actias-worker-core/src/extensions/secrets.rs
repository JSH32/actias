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
    pub secret_client: Option<SecretServiceClient<crate::Grpc>>,
    pub project_id: String,
    /// Script whose vms declare; audit metadata on every resolution.
    pub script_id: String,
    /// A workflow run's pins: the first resolution of a name records the
    /// version it saw, every later vm build resolves exactly that
    /// version, and a rotation mid-run cannot diverge replay. [`None`]
    /// in every other vm resolves heads.
    pub pins: Option<std::sync::Arc<crate::platform::workflow::SecretPins>>,
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
        let pins = self.pins.clone();

        // `local token = secret "name"`: the handle is the string value
        // itself, resolved at declaration.
        let declaration = lua.create_async_function(move |lua, name: String| {
            let secret_client = secret_client.clone();
            let project_id = project_id.clone();
            let script_id = script_id.clone();
            let pins = pins.clone();

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

                let pinned = pins.as_ref().and_then(|pins| pins.version_for(&name));
                let resolved = secret_client
                    .resolve_secret(ResolveSecretRequest {
                        project_id,
                        name: name.clone(),
                        // 0 is the head; a pinned run asks for exactly the
                        // version its first resolution saw.
                        version: pinned.unwrap_or(0),
                        script_id,
                    })
                    .await;

                match resolved {
                    Ok(resolved) => {
                        let resolved = resolved.into_inner();
                        if let Some(pins) = pins
                            && pinned.is_none()
                        {
                            // Durable before use: an unpersisted pin could
                            // replay as a different value.
                            pins.record(&name, resolved.version)
                                .map_err(mlua::Error::RuntimeError)?;
                        }
                        Ok(resolved.value)
                    }
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
