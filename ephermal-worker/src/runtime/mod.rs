pub mod module;
pub mod request;
pub mod response;

use self::module::LuaModule;
use mlua::{Lua, LuaSerdeExt, Table};
use std::ops::Deref;
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
    pub fn new() -> mlua::Result<Self> {
        trace!("Initializing lua runtime");

        let lua = Self(Lua::new_with(
            mlua::StdLib::ALL_SAFE,
            mlua::LuaOptions::new().catch_rust_panics(false),
        )?);

        lua.sandbox(true)?;

        // Json operations for serializing and deserializing lua values.
        let json_methods = lua.create_table()?;
        json_methods.set(
            "stringify",
            lua.create_function(|_lua, value: mlua::Value| {
                Ok(serde_json::to_string(&value)
                    .map_err(|e| mlua::Error::SerializeError(e.to_string()))?)
            })?,
        )?;

        json_methods.set(
            "parse",
            lua.create_function(|lua, string: mlua::String| {
                Ok(lua.to_value(
                    &serde_json::from_str::<serde_json::Value>(string.to_str()?)
                        .map_err(|e| mlua::Error::DeserializeError(e.to_string()))?,
                )?)
            })?,
        )?;

        lua.globals().set("json", json_methods.clone())?;

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
            lua.create_function(|lua, module_name: String| {
                Ok(lua
                    .named_registry_value::<_, mlua::Table>("module_registry")?
                    .get::<_, mlua::Value>(module_name)?)
            })?,
        )?;

        trace!("Initializing module registry");
        lua.set_named_registry_value("module_registry", lua.create_table()?)?;

        // Now we can set everything in the registry
        lua.set_module("json", mlua::Value::Table(json_methods))?;

        Ok(lua)
    }

    /// Set a module from an object.
    pub fn set_module(&self, key: &str, value: mlua::Value) -> mlua::Result<()> {
        let registry: Table = self.named_registry_value("module_registry")?;
        registry.set(key, value)?;
        self.set_named_registry_value("module_registry", registry)?;

        trace!(name = key, "Registered lua module");

        Ok(())
    }

    /// Set a module from a [`LuaModule`].
    pub async fn set_module_from_source(&self, module: LuaModule) -> mlua::Result<()> {
        self.set_module(&module.name, self.load(&module).call_async(()).await?)?;

        Ok(())
    }
}
