use crate::runtime::extension::{ExtensionInfo, LuaExtension};
use mlua::{ExternalResult, LuaSerdeExt, UserData};
use std::sync::{Arc, RwLock};
use wasmer::{Extern, Imports, Instance, Module, Store};

pub struct WasmExtension;

impl LuaExtension for WasmExtension {
    fn extension_info(&self) -> crate::runtime::extension::ExtensionInfo {
        ExtensionInfo {
            name: "wasm",
            description: "Webassembly runtime",
            default: true,
        }
    }

    fn create_extension<'a>(&'a self, lua: &'a mlua::Lua) -> mlua::Result<mlua::Value> {
        let wasm = lua.create_table()?;

        wasm.set(
            "from",
            lua.create_function(|lua, binary: Vec<u8>| {
                let instance = WasmInstance::new(binary).to_lua_err()?;
                Ok(lua.create_userdata(instance))
            })?,
        )?;

        let wasm_class = lua.create_proxy::<WasmInstance>()?;
        wasm.set("WasmInstance", wasm_class.clone())?;
        lua.globals().set("WasmInstance", wasm_class)?;

        Ok(mlua::Value::Table(wasm))
    }
}
struct WasmInstance(Arc<RwLock<InnerInstance>>);
struct InnerInstance {
    wasm_instance: Instance,
    store: Store,
}

impl WasmInstance {
    pub fn new(binary: Vec<u8>) -> Result<Self, String> {
        let mut store = Store::default();

        let module = Module::new(&store, binary).map_err(|e| e.to_string())?;

        let imports = Imports::new();

        let wasm_instance =
            Instance::new(&mut store, &module, &imports).map_err(|e| e.to_string())?;

        // let load: TypedFunction<(), (WasmPtr<u8>, i32)> = wasm_instance
        //     .exports
        //     .get_typed_function(&mut store, "load")
        //     .map_err(|e| e.to_string())?;

        Ok(Self(Arc::new(RwLock::new(InnerInstance {
            store,
            wasm_instance,
            // mem_load: load,
        }))))
    }
}

impl UserData for WasmInstance {
    fn add_fields<'lua, F: mlua::UserDataFields<'lua, Self>>(_fields: &mut F) {}

    fn add_methods<'lua, M: mlua::UserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_method_mut("get_export", |lua, this, export: String| {
            Ok(
                match this
                    .0
                    .read()
                    .unwrap()
                    .wasm_instance
                    .exports
                    .get_extern(&export)
                {
                    Some(v) => match v {
                        Extern::Function(v) => wasm_to_lua(
                            &lua,
                            this.0.clone(),
                            wasmer::Value::FuncRef(Some(v.to_owned())),
                        )?,
                        Extern::Global(_) => todo!(),
                        Extern::Table(_) => todo!(),
                        Extern::Memory(_) => todo!(),
                    },
                    None => mlua::Value::Nil,
                },
            )
        });
    }
}

struct WasmValue {
    value: wasmer::Value,
    wasm: Arc<RwLock<InnerInstance>>,
}

impl UserData for WasmValue {
    fn add_methods<'lua, M: mlua::UserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_meta_method("__call", |lua, this, args: mlua::Variadic<mlua::Value>| {
            if let wasmer::Value::FuncRef(Some(func)) = &this.value {
                let mut params = vec![];

                for arg in args {
                    params.push(lua_to_wasm(arg.to_owned())?);
                }

                let result = func
                    .call(&mut this.wasm.write().unwrap().store, params.as_slice())
                    .to_lua_err()?;

                Ok(wasm_to_lua(lua, this.wasm.clone(), (*result)[0].clone()))
            } else {
                Err(mlua::Error::RuntimeError(
                    "Webassembly value was not a function".into(),
                ))
            }
        });
    }
}

fn wasm_to_lua(
    lua: &mlua::Lua,
    inner: Arc<RwLock<InnerInstance>>,
    value: wasmer::Value,
) -> mlua::Result<mlua::Value> {
    Ok(match value {
        wasmer::Value::I32(v) => lua.to_value(&v)?,
        wasmer::Value::I64(v) => lua.to_value(&v)?,
        wasmer::Value::F32(v) => lua.to_value(&v)?,
        wasmer::Value::F64(v) => lua.to_value(&v)?,
        wasmer::Value::V128(v) => lua.to_value(&v)?,
        wasmer::Value::ExternRef(v) => match v {
            Some(v) => mlua::Value::UserData(lua.create_userdata(WasmValue {
                value: wasmer::Value::ExternRef(Some(v)),
                wasm: inner,
            })?),
            None => mlua::Value::Nil,
        },
        wasmer::Value::FuncRef(v) => match v {
            Some(v) => mlua::Value::UserData(lua.create_userdata(WasmValue {
                value: wasmer::Value::FuncRef(Some(v)),
                wasm: inner,
            })?),
            None => mlua::Value::Nil,
        },
    })
}

fn lua_to_wasm(value: mlua::Value) -> mlua::Result<wasmer::Value> {
    Ok(match value {
        mlua::Value::Nil => wasmer::Value::I32(0),
        mlua::Value::Boolean(v) => wasmer::Value::I32(v as i32),
        mlua::Value::Integer(v) => wasmer::Value::I32(v),
        mlua::Value::Number(v) => wasmer::Value::F64(v),
        // Types that can't be converted.
        mlua::Value::Vector(_, _, _)
        | mlua::Value::String(_)
        | mlua::Value::Table(_)
        | mlua::Value::Function(_)
        | mlua::Value::Thread(_)
        | mlua::Value::LightUserData(_)
        | mlua::Value::UserData(_)
        | mlua::Value::Error(_) => {
            return Err(mlua::Error::RuntimeError(
                "Argument cannot be converted to Webassembly value.".into(),
            ))
        }
    })
}
