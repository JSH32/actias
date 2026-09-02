//! The trait every capability implements to reach a vm, and how one
//! names itself. Every new capability is an extension; nothing is
//! bolted onto the runtime directly.

/// How one native extension names itself to the runtime.
pub struct ExtensionInfo<'a> {
    pub name: &'a str,
    pub description: &'a str,
    /// Whether the runtime installs this extension as a global
    /// without the script asking for it.
    pub default: bool,
}

pub trait LuaExtension {
    /// Creates the extension's Lua value, installed under
    /// [`ExtensionInfo::name`].
    fn create_extension(&self, lua: &mlua::Lua) -> mlua::Result<mlua::Value>;

    /// Returns the name of the extension
    fn extension_info(&self) -> ExtensionInfo<'_>;
}

/// Standard extensions that are always included.
pub mod standard_extensions {
    use mlua::LuaSerdeExt;

    use super::*;

    pub struct JsonExtension;

    impl LuaExtension for JsonExtension {
        fn create_extension(&self, lua: &mlua::Lua) -> mlua::Result<mlua::Value> {
            let json = lua.create_table()?;

            json.set(
                "stringify",
                lua.create_function(|_lua, value: mlua::Value| {
                    serde_json::to_string(&value)
                        .map_err(|e| mlua::Error::SerializeError(e.to_string()))
                })?,
            )?;

            json.set(
                "parse",
                lua.create_function(|lua, string: mlua::String| {
                    lua.to_value(
                        &serde_json::from_str::<serde_json::Value>(&string.to_str()?)
                            .map_err(|e| mlua::Error::DeserializeError(e.to_string()))?,
                    )
                })?,
            )?;

            Ok(mlua::Value::Table(json))
        }

        fn extension_info(&self) -> ExtensionInfo<'_> {
            ExtensionInfo {
                name: "json",
                description: "Operations for creating/parsing JSON data.",
                default: true,
            }
        }
    }

    pub struct UuidExtension;

    impl LuaExtension for UuidExtension {
        fn create_extension(&self, lua: &mlua::Lua) -> mlua::Result<mlua::Value> {
            let uuid = lua.create_table()?;

            uuid.set(
                "v4",
                lua.create_function(|_lua, _: ()| Ok(uuid::Uuid::new_v4().to_string()))?,
            )?;

            Ok(mlua::Value::Table(uuid))
        }

        fn extension_info(&self) -> ExtensionInfo<'_> {
            ExtensionInfo {
                name: "uuid",
                description: "UUID module for generating UUIDs.",
                default: true,
            }
        }
    }
}
