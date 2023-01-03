pub mod extension;

use crate::runtime::extension::standard_extensions::JsonExtension;

use self::extension::LuaExtension;
use ephermal_common::models::Bundle;
use mlua::{AsChunk, ExternalResult, Lua, Table};
use std::{borrow::Cow, ops::Deref};
use tracing::trace;

/// Lua runtime with ephermal specific methods.
pub struct EphermalRuntime(Lua);

impl Deref for EphermalRuntime {
    type Target = Lua;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl EphermalRuntime {
    pub async fn new(bundle: Option<Bundle>) -> mlua::Result<Self> {
        trace!("Initializing lua runtime");

        let lua = Self(Lua::new_with(
            mlua::StdLib::ALL_SAFE,
            mlua::LuaOptions::new().catch_rust_panics(false),
        )?);

        lua.set_app_data::<Option<Bundle>>(bundle.clone());

        lua.sandbox(true)?;

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
                let bundle = lua.app_data_ref::<Option<Bundle>>().unwrap();

                if let Some(bundle) = bundle.as_ref() {
                    let file = bundle.files.iter().find(|file| {
                        let path = file.path.replace("/", ".");
                        path == module_name
                            || path == format!("{}.lua", module_name)
                            || path == module_name.strip_suffix(".lua").unwrap_or(&module_name)
                    });

                    if let Some(file) = file {
                        let result: mlua::Value = lua
                            .load(&LuaModule {
                                name: file.file_name.clone(),
                                source: std::str::from_utf8(&file.content)
                                    .to_lua_err()?
                                    .to_string(),
                            })
                            .eval_async()
                            .await?;

                        let registry: Table = lua.named_registry_value("module_registry")?;
                        registry.set(file.file_name.clone(), result.clone())?;
                        lua.set_named_registry_value("module_registry", registry)?;

                        return Ok(result);
                    }
                }

                Ok(lua
                    .named_registry_value::<_, mlua::Table>("module_registry")?
                    .get::<_, mlua::Value>(module_name)?)
            })?,
        )?;

        // Like require but doesn't put anything in the module registry.
        lua.globals().set(
            "dofile",
            lua.create_async_function(|lua, module_name: String| async move {
                let bundle = lua.app_data_ref::<Option<Bundle>>().unwrap();

                if let Some(bundle) = bundle.as_ref() {
                    let file = bundle.files.iter().find(|file| file.path == module_name);

                    if let Some(file) = file {
                        return Ok(lua
                            .load(&LuaModule {
                                name: file.file_name.clone(),
                                source: std::str::from_utf8(&file.content)
                                    .to_lua_err()?
                                    .to_string(),
                            })
                            .eval_async()
                            .await?);
                    }
                }

                Ok(mlua::Value::Nil)
            })?,
        )?;

        trace!("Initializing module registry");
        lua.set_named_registry_value("module_registry", lua.create_table()?)?;

        lua.register_extensions(&[&JsonExtension, &crate::extensions::http::HttpExtension])?;

        if let Some(bundle) = bundle {
            let entry_point = bundle
                .files
                .iter()
                .find(|file| file.file_name == bundle.entry_point);

            if let Some(entry_point) = entry_point {
                lua.load(&LuaModule {
                    name: entry_point.file_name.clone(),
                    source: std::str::from_utf8(&entry_point.content)
                        .to_lua_err()?
                        .to_string(),
                })
                .eval_async()
                .await?;
            }
        }

        Ok(lua)
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

            let extension = extension.create_extension(&self.0)?;
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

    /// Set a module from a [`LuaModule`].
    pub async fn set_module_from_source(&self, module: LuaModule) -> mlua::Result<()> {
        self.set_module(&module.name, self.load(&module).call_async(()).await?)?;

        Ok(())
    }
}

/// Lua module.
pub struct LuaModule {
    /// Name or path of the module.
    pub name: String,
    /// Source code of the module.
    pub source: String,
}

impl AsChunk<'_> for LuaModule {
    fn source(&self) -> std::io::Result<std::borrow::Cow<[u8]>> {
        Ok(Cow::Owned(self.source.as_bytes().to_vec()))
    }

    fn name(&self) -> Option<String> {
        Some(self.name.clone())
    }
}
