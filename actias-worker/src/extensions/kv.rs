use mlua::{IntoLua, LuaSerdeExt, UserData};
use tonic::Code;

use crate::{
    proto::kv_service::{
        kv_service_client::KvServiceClient, DeletePairsRequest, Pair, PairRequest, SetPairsRequest,
        ValueType,
    },
    runtime::extension::{ExtensionInfo, LuaExtension},
};

pub struct KvExtension {
    pub kv_client: KvServiceClient<tonic::transport::Channel>,
    pub project_id: String,
}

impl LuaExtension for KvExtension {
    fn create_extension<'a>(&'a self, lua: &'a mlua::Lua) -> mlua::Result<mlua::Value> {
        let kv = lua.create_table()?;

        let kv_client = self.kv_client.clone();
        let project_id = self.project_id.clone();

        kv.set(
            "get_namespace",
            lua.create_function(move |lua, namespace: String| {
                Ok(lua.create_userdata(KvNamespace {
                    kv_client: kv_client.clone(),
                    project_id: project_id.clone(),
                    namespace: namespace,
                })?)
            })?,
        )?;

        Ok(mlua::Value::Table(kv))
    }

    fn extension_info(&self) -> ExtensionInfo {
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
                    match pair.r#type() {
                        ValueType::String => pair.value.into_lua(lua),
                        ValueType::Number => pair.value.parse::<f64>().unwrap().into_lua(lua),
                        ValueType::Integer => pair.value.parse::<i64>().unwrap().into_lua(lua),
                        ValueType::Boolean => match pair.value.as_str() {
                            "true" => true,
                            "false" => false,
                            _ => {
                                return Err(mlua::Error::RuntimeError(
                                    "Boolean value was not a boolean".into(),
                                ))
                            }
                        }
                        .into_lua(lua),
                        ValueType::Json => lua.to_value(
                            &serde_json::from_str::<serde_json::Value>(&pair.value)
                                .map_err(|e| mlua::Error::DeserializeError(e.to_string()))?,
                        ),
                    }?
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
                let (val_type, val) = match value {
                    mlua::Value::Nil => {
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
                        ))
                    }
                };

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

                Ok(())
            },
        )
    }
}
