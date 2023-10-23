/// Descriptions for a native based extension.
pub struct ExtensionInfo<'a> {
    /// Name of the extension
    pub name: &'a str,
    /// Description of the extension.
    pub description: &'a str,
    /// Should the extension be registered as a global by default.
    pub default: bool,
}

pub trait LuaExtension {
    /// Create the extension and return a corresponding value.
    fn create_extension<'a>(&'a self, lua: &'a mlua::Lua) -> mlua::Result<mlua::Value>;

    /// Returns the name of the extension
    fn extension_info(&self) -> ExtensionInfo;
}

/// Standard extensions that are always included.
pub mod standard_extensions {
    use mlua::LuaSerdeExt;

    use super::*;

    pub struct JsonExtension;

    impl LuaExtension for JsonExtension {
        fn create_extension<'a>(&'a self, lua: &'a mlua::Lua) -> mlua::Result<mlua::Value> {
            let json = lua.create_table()?;

            json.set(
                "stringify",
                lua.create_function(|_lua, value: mlua::Value| {
                    Ok(serde_json::to_string(&value)
                        .map_err(|e| mlua::Error::SerializeError(e.to_string()))?)
                })?,
            )?;

            json.set(
                "parse",
                lua.create_function(|lua, string: mlua::String| {
                    Ok(lua.to_value(
                        &serde_json::from_str::<serde_json::Value>(string.to_str()?)
                            .map_err(|e| mlua::Error::DeserializeError(e.to_string()))?,
                    )?)
                })?,
            )?;

            Ok(mlua::Value::Table(json))
        }

        fn extension_info(&self) -> ExtensionInfo {
            ExtensionInfo {
                name: "json",
                description: "Operations for creating/parsing JSON data.",
                default: true,
            }
        }
    }

    pub struct UuidExtension;

    impl LuaExtension for UuidExtension {
        fn create_extension<'a>(&'a self, lua: &'a mlua::Lua) -> mlua::Result<mlua::Value> {
            let uuid = lua.create_table()?;

            uuid.set(
                "v4",
                lua.create_function(|_lua, _: ()| Ok(uuid::Uuid::new_v4().to_string()))?,
            )?;

            Ok(mlua::Value::Table(uuid))
        }

        fn extension_info(&self) -> ExtensionInfo {
            ExtensionInfo {
                name: "uuid",
                description: "UUID module for generating UUIDs.",
                default: true,
            }
        }
    }
}
