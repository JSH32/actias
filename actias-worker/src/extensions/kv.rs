use mlua::{IntoLua, LuaSerdeExt, UserData};
use tonic::Code;

use crate::{
    proto::kv_service::{
        DeletePairsRequest, Pair, PairRequest, SetPairsRequest, ValueType,
        kv_service_client::KvServiceClient,
    },
    runtime::extension::{ExtensionInfo, LuaExtension},
};

pub struct KvExtension {
    pub kv_client: KvServiceClient<tonic::transport::Channel>,
    pub project_id: String,
}

impl LuaExtension for KvExtension {
    fn create_extension<'a>(&'a self, lua: &'a mlua::Lua) -> mlua::Result<mlua::Value<'a>> {
        let kv = lua.create_table()?;

        let kv_client = self.kv_client.clone();
        let project_id = self.project_id.clone();

        kv.set(
            "get_namespace",
            lua.create_function(move |lua, namespace: String| {
                lua.create_userdata(KvNamespace {
                    kv_client: kv_client.clone(),
                    project_id: project_id.clone(),
                    namespace,
                })
            })?,
        )?;

        Ok(mlua::Value::Table(kv))
    }

    fn extension_info(&self) -> ExtensionInfo<'_> {
        ExtensionInfo {
            name: "kv",
            description: "Key Value store for persistent data",
            default: true,
        }
    }
}

#[derive(Clone)]
pub struct KvNamespace {
    kv_client: KvServiceClient<tonic::transport::Channel>,
    namespace: String,
    project_id: String,
}

/// Converts a stored pair into the lua value its declared type describes.
///
/// # Arguments
/// * `value_type` - Type the pair was stored as.
/// * `value` - Stored text, which the service keeps for every type.
///
/// # Errors
/// Returns [`mlua::Error::RuntimeError`] when the text does not parse as the
/// declared type. That means the row disagrees with its own metadata, so it is
/// reported to the script rather than being allowed to abort the request.
fn pair_into_lua<'lua>(
    lua: &'lua mlua::Lua,
    value_type: ValueType,
    value: &str,
) -> mlua::Result<mlua::Value<'lua>> {
    let mismatch = |expected: &str| {
        mlua::Error::RuntimeError(format!("Stored value is not a valid {expected}."))
    };

    match value_type {
        ValueType::String => value.to_owned().into_lua(lua),
        ValueType::Number => value
            .parse::<f64>()
            .map_err(|_| mismatch("number"))?
            .into_lua(lua),
        ValueType::Integer => value
            .parse::<i64>()
            .map_err(|_| mismatch("integer"))?
            .into_lua(lua),
        ValueType::Boolean => value
            .parse::<bool>()
            .map_err(|_| mismatch("boolean"))?
            .into_lua(lua),
        ValueType::Json => lua.to_value(
            &serde_json::from_str::<serde_json::Value>(value).map_err(|_| mismatch("json"))?,
        ),
    }
}

impl UserData for KvNamespace {
    fn add_methods<'lua, M: mlua::UserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_async_method_mut("get", |lua, this, key: String| async {
            let pair = match this
                .kv_client
                .get_pair(PairRequest {
                    project_id: this.project_id.clone(),
                    namespace: this.namespace.clone(),
                    key,
                })
                .await
            {
                Ok(v) => {
                    let pair = v.into_inner();
                    let value_type = pair.r#type();

                    pair_into_lua(lua, value_type, &pair.value)?
                }
                Err(e) => {
                    if e.code() == Code::NotFound {
                        return Ok(mlua::Value::Nil);
                    } else {
                        return Err(mlua::Error::RuntimeError(e.message().to_string()));
                    }
                }
            };

            Ok(pair)
        });

        methods.add_async_method_mut(
            "set",
            |_, this, (key, value): (String, mlua::Value)| async {
                match value.into_service_value()? {
                    Some((val_type, val)) => {
                        this.kv_client
                            .set_pairs(SetPairsRequest {
                                pairs: vec![Pair {
                                    project_id: this.project_id.clone(),
                                    namespace: this.namespace.clone(),
                                    r#type: val_type.into(),
                                    ttl: None,
                                    key,
                                    value: val,
                                }],
                            })
                            .await
                            .map_err(|e| mlua::Error::RuntimeError(e.message().to_string()))?;
                    }
                    None => {
                        // We delete
                        this.kv_client
                            .delete_pairs(DeletePairsRequest {
                                pairs: vec![PairRequest {
                                    project_id: this.project_id.clone(),
                                    namespace: this.namespace.clone(),
                                    key,
                                }],
                            })
                            .await
                            .map_err(|e| mlua::Error::RuntimeError(e.message().to_string()))?;
                        return Ok(());
                    }
                };

                Ok(())
            },
        );

        methods.add_async_method_mut("set_batch", |_, this, values: mlua::Table| async {
            let mut to_set = vec![];
            let mut to_delete = vec![];

            for pair in values.pairs::<String, mlua::Value>() {
                let (key, value) = pair?;

                match value.into_service_value()? {
                    Some((val_type, val)) => to_set.push(Pair {
                        project_id: this.project_id.clone(),
                        namespace: this.namespace.clone(),
                        r#type: val_type.into(),
                        ttl: None,
                        key,
                        value: val,
                    }),
                    None => to_delete.push(PairRequest {
                        project_id: this.project_id.clone(),
                        namespace: this.namespace.clone(),
                        key,
                    }),
                }
            }

            if !to_set.is_empty() {
                this.kv_client
                    .set_pairs(SetPairsRequest { pairs: to_set })
                    .await
                    .map_err(|e| mlua::Error::RuntimeError(e.message().to_string()))?;
            }

            if !to_delete.is_empty() {
                this.kv_client
                    .delete_pairs(DeletePairsRequest { pairs: to_delete })
                    .await
                    .map_err(|e| mlua::Error::RuntimeError(e.message().to_string()))?;
            }

            Ok(())
        });

        methods.add_async_method_mut("delete", |_, this, keys: mlua::MultiValue| async {
            let keys: Vec<PairRequest> = keys
                .into_vec()
                .into_iter()
                .map(|key| {
                    key.to_string().map(|string_key| PairRequest {
                        project_id: this.project_id.clone(),
                        namespace: this.namespace.clone(),
                        key: string_key,
                    })
                })
                .collect::<mlua::Result<Vec<_>>>()?;

            this.kv_client
                .delete_pairs(DeletePairsRequest { pairs: keys })
                .await
                .map_err(|e| mlua::Error::RuntimeError(e.message().to_string()))?;

            Ok(())
        })
    }
}

trait KvValue {
    /// Converts this value into representation of the value with the stringified value.
    /// If the [`Option`] is [`None`], then the value should be deleted.
    fn into_service_value(self) -> Result<Option<(ValueType, String)>, mlua::Error>;
}

impl KvValue for mlua::Value<'_> {
    fn into_service_value(self) -> Result<Option<(ValueType, String)>, mlua::Error> {
        Ok(Some(match self {
            mlua::Value::Nil => {
                return Ok(None);
            }
            mlua::Value::Boolean(v) => (ValueType::Boolean, v.to_string()),
            mlua::Value::Integer(v) => (ValueType::Integer, v.to_string()),
            mlua::Value::Number(v) => (ValueType::Number, v.to_string()),
            mlua::Value::String(v) => (ValueType::String, v.to_str().unwrap().to_owned()),
            mlua::Value::Table(v) => (
                ValueType::Json,
                serde_json::to_string(&v)
                    .map_err(|e| mlua::Error::SerializeError(e.to_string()))?,
            ),
            mlua::Value::Vector(v) => (
                ValueType::Json,
                serde_json::to_string(&v)
                    .map_err(|e| mlua::Error::SerializeError(e.to_string()))?,
            ),
            _ => {
                return Err(mlua::Error::SerializeError(
                    "Invalid datatype provided".to_owned(),
                ));
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_type_converts_to_its_lua_counterpart() {
        let lua = mlua::Lua::new();

        match pair_into_lua(&lua, ValueType::String, "hello").unwrap() {
            mlua::Value::String(v) => assert_eq!(v.to_str().unwrap(), "hello"),
            other => panic!("expected a string, got {other:?}"),
        }
        match pair_into_lua(&lua, ValueType::Integer, "42").unwrap() {
            mlua::Value::Integer(v) => assert_eq!(v, 42),
            other => panic!("expected an integer, got {other:?}"),
        }
        match pair_into_lua(&lua, ValueType::Number, "1.5").unwrap() {
            mlua::Value::Number(v) => assert_eq!(v, 1.5),
            other => panic!("expected a number, got {other:?}"),
        }
        match pair_into_lua(&lua, ValueType::Boolean, "true").unwrap() {
            mlua::Value::Boolean(v) => assert!(v),
            other => panic!("expected a boolean, got {other:?}"),
        }
        match pair_into_lua(&lua, ValueType::Json, r#"{"a":1}"#).unwrap() {
            mlua::Value::Table(_) => {}
            other => panic!("expected a table, got {other:?}"),
        }
    }

    #[test]
    fn a_value_disagreeing_with_its_type_errors_instead_of_panicking() {
        let lua = mlua::Lua::new();

        // A row whose text does not match its stored type is corrupt data, and
        // corrupt data is an expected input on the request path.
        for (value_type, value) in [
            (ValueType::Number, "not a number"),
            (ValueType::Integer, "1.5"),
            (ValueType::Boolean, "yes"),
            (ValueType::Json, "{unclosed"),
        ] {
            let result = pair_into_lua(&lua, value_type, value);
            assert!(
                result.is_err(),
                "{value_type:?} with {value:?} should be an error"
            );
        }
    }
}
