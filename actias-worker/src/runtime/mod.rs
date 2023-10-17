pub mod extension;

use crate::{
    extensions::kv::KvExtension,
    proto::{
        bundle::Bundle,
        kv_service::kv_service_client::KvServiceClient,
        script_service::{Revision, Script},
    },
    runtime::extension::standard_extensions::JsonExtension,
};

use self::extension::LuaExtension;
use actias_common::tracing::trace;
use mlua::{AsChunk, ExternalResult, Lua, LuaSerdeExt, Table, UserData};
use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
    ops::Deref,
    sync::{Arc, RwLock},
    time::Instant,
};

/// Lua runtime with actias specific methods.
pub struct ActiasRuntime {
    lua: Lua,
    // Arc to pass to interrupt handler.
    timer: Arc<RwLock<Timer>>,
}

#[derive(Clone)]
struct Timer {
    start_time: Option<Instant>,
    time_limit: Option<u64>,
}

impl Deref for ActiasRuntime {
    type Target = Lua;

    fn deref(&self) -> &Self::Target {
        &self.lua
    }
}

/// Script info table exposed to lua.
#[derive(Serialize, Deserialize, Clone, Debug)]
struct ScriptInfo {
    /// Public script identifier
    identifier: String,
    project_id: String,
}

impl UserData for ScriptInfo {}

impl ActiasRuntime {
    /// Create a new [`ActiasRuntime`], this will run the main script from the entrypoint defined in the [`Bundle`].
    ///
    /// # Arguments
    /// - `script` - Script information, this is so the script can identify it's own routing pattern.
    /// - `revision` - Script revision, ensure that this revision has a [`Bundle`] (use `with_bundle`).
    /// - `kv_client` - Key value service client, allows the script to access/store persistent data.
    /// - `time_limit` - Total Time limit in seconds, this is based on seconds and will start when [`start_timer`] is called
    pub async fn new(
        script: Script,
        revision: Revision,
        kv_client: KvServiceClient<tonic::transport::Channel>,
        time_limit: Option<u64>,
    ) -> mlua::Result<Self> {
        trace!("Initializing lua runtime");

        let lua = Self {
            lua: Lua::new_with(
                mlua::StdLib::ALL_SAFE,
                mlua::LuaOptions::new().catch_rust_panics(false),
            )?,
            timer: Arc::new(RwLock::new(Timer {
                start_time: None,
                time_limit,
            })),
        };

        let bundle = revision.bundle.unwrap();
        lua.set_app_data::<Bundle>(bundle.clone());

        lua.sandbox(true)?;

        // 128 MB memory limit.
        lua.set_memory_limit(128 * 1000000)?;

        let timer = lua.timer.clone();

        // Time limit each worker total runtime.
        // TODO: Figure out how to make this CPU time based.
        lua.set_interrupt(move |_| {
            let timer = timer.read().unwrap();
            if let (Some(start_time), Some(time_limit)) = (timer.start_time, timer.time_limit) {
                if Instant::now().duration_since(start_time).as_secs() > time_limit {
                    return Err(mlua::Error::RuntimeError(format!(
                        "Script timed out, limit is {} seconds.",
                        time_limit
                    )));
                }
            }

            Ok(mlua::VmState::Continue)
        });

        // Function to add listener to registry
        // All added listeners are prefixed with `_listener`
        lua.globals().set(
            "add_event_listener",
            lua.create_function(|lua, (event, callback): (String, mlua::Function)| {
                // All event names should be in this array.
                if !["fetch"].contains(&event.as_str()) {
                    Err(mlua::Error::RuntimeError("Invalid event specified.".into()))
                } else {
                    lua.set_named_registry_value(&format!("listener_{event}"), callback)?;
                    Ok(())
                }
            })?,
        )?;

        // Global function to retrieve modules from the registry.
        lua.globals().set(
            "require",
            lua.create_async_function(|lua, module_name: String| async move {
                let bundle = lua.app_data_ref::<Bundle>().unwrap();

                let file = bundle.files.iter().find(|file| {
                    let path = file.file_path.replace("/", ".");
                    path == module_name
                        || path == format!("{}.lua", module_name)
                        || path == module_name.strip_suffix(".lua").unwrap_or(&module_name)
                });

                if let Some(file) = file {
                    let result: mlua::Value = lua
                        .load(&LuaModule {
                            name: file.file_path.clone(),
                            source: std::str::from_utf8(&file.content)
                                .into_lua_err()?
                                .to_string(),
                        })
                        .eval_async()
                        .await?;

                    let registry: Table = lua.named_registry_value("module_registry")?;
                    registry.set(file.file_name.clone(), result.clone())?;
                    lua.set_named_registry_value("module_registry", registry)?;

                    return Ok(result);
                }

                Ok(lua
                    .named_registry_value::<mlua::Table>("module_registry")?
                    .get::<_, mlua::Value>(module_name)?)
            })?,
        )?;

        // Like require but doesn't put anything in the module registry.
        lua.globals().set(
            "dofile",
            lua.create_async_function(|lua, module_name: String| async move {
                let bundle = lua.app_data_ref::<Bundle>().unwrap();

                let file = bundle
                    .files
                    .iter()
                    .find(|file| file.file_path == module_name);

                if let Some(file) = file {
                    return Ok(lua
                        .load(&LuaModule {
                            name: file.file_name.clone(),
                            source: std::str::from_utf8(&file.content)
                                .into_lua_err()?
                                .to_string(),
                        })
                        .eval_async()
                        .await?);
                }

                Ok(mlua::Value::Nil)
            })?,
        )?;

        lua.globals().set(
            "getfile",
            lua.create_function(|lua, path: String| {
                let bundle = lua.app_data_ref::<Bundle>().unwrap();
                let file = bundle.files.iter().find(|file| file.file_path == path);

                Ok(match file {
                    Some(v) => lua.to_value(&v.content)?,
                    None => mlua::Value::Nil,
                })
            })?,
        )?;

        trace!("Initializing module registry");
        lua.set_named_registry_value("module_registry", lua.create_table()?)?;

        lua.register_extensions(&[
            &JsonExtension,
            &crate::extensions::http::HttpExtension,
            &KvExtension {
                kv_client,
                project_id: script.project_id.clone(),
            },
        ])?;

        lua.globals().set(
            "script",
            lua.to_value(&ScriptInfo {
                identifier: script.public_identifier,
                project_id: script.project_id,
            })?,
        )?;

        let entry_point = bundle
            .files
            .iter()
            .find(|file| file.file_name == bundle.entry_point);

        // We need to set a new timer temporrily when registering.
        // This should be one second since nothing should be happening in this time.
        let original_timer = lua.timer.read().unwrap().clone();
        *lua.timer.write().unwrap() = Timer {
            start_time: Some(Instant::now()),
            time_limit: Some(1),
        };

        // Run entry point and register handlers.
        if let Some(entry_point) = entry_point {
            lua.load(&LuaModule {
                name: entry_point.file_name.clone(),
                source: std::str::from_utf8(&entry_point.content)
                    .into_lua_err()?
                    .to_string(),
            })
            .eval_async()
            .await?;
        }

        // Set original timer.
        *lua.timer.write().unwrap() = original_timer;

        Ok(lua)
    }

    /// Start the timer. This only works if `time_limit` is set.
    /// This will stop the runtime with an error once the time limit has passed since the timer has been started.
    pub fn start_timer(&self) {
        let mut timer = self.timer.write().unwrap();
        if let Some(_) = timer.time_limit {
            timer.start_time = Some(Instant::now());
        }
    }

    /// Register an extension into the runtime.
    pub fn register_extensions(&self, extensions: &[&dyn LuaExtension]) -> mlua::Result<()> {
        for extension in extensions {
            let info = extension.extension_info();

            trace!(
                name = info.name,
                description = info.description,
                "Registering extension"
            );

            let extension = extension.create_extension(&self)?;
            self.set_module(&info.name, extension.clone())?;

            // Register extension as a global
            if info.default {
                self.globals().set(info.name, extension)?;
            }
        }

        Ok(())
    }

    /// Set a module from an object.
    pub fn set_module(&self, key: &str, value: mlua::Value) -> mlua::Result<()> {
        let registry: Table = self.named_registry_value("module_registry")?;
        registry.set(key, value)?;
        self.set_named_registry_value("module_registry", registry)?;

        trace!(module_name = key, "Registered to module registry");

        Ok(())
    }

    // /// Set a module from a [`LuaModule`].
    // pub async fn set_module_from_source(&self, module: LuaModule) -> mlua::Result<()> {
    //     self.set_module(&module.name, self.load(&module).call_async(()).await?)?;

    //     Ok(())
    // }
}

/// Lua module.
pub struct LuaModule {
    /// Name or path of the module.
    pub name: String,
    /// Source code of the module.
    pub source: String,
}

impl AsChunk<'_, '_> for &LuaModule {
    fn source(self) -> std::io::Result<Cow<'static, [u8]>> {
        Ok(Cow::Owned(self.source.as_bytes().to_vec()))
    }

    fn name(&self) -> Option<String> {
        Some(self.name.clone())
    }
}
