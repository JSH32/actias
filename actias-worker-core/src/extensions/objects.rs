//! Durable object declarations and calls.
//!
//! `local Room = object "Room" { ... }` declares a class at the top level
//! and returns its handle; `objects "Room"` references one declared
//! elsewhere. `Room:get("lobby")` mints an instance handle whose method
//! calls travel through the node's object router to the instance's pinned
//! vm, one mailbox message per call, arguments and returns as plain
//! serializable values.
//!
//! Every vm also carries `__dispatch`, the receiving side: it resolves the
//! class method registered in this vm and runs it with the object's state
//! table first. A request vm never dispatches; the pinned vm the router
//! spawned does.

use std::sync::Arc;

use mlua::{Lua, LuaSerdeExt, Table};
use serde::Deserialize;

use crate::runtime::extension::{ExtensionInfo, LuaExtension};
use crate::runtime::{ActiasRuntime, ContractKind};

/// Registry key of the class-name-to-methods table in this vm.
const CLASSES_KEY: &str = "object_classes";

/// Registry key of the object's state table; exists only in pinned vms,
/// created on their first dispatched call.
const STATE_KEY: &str = "object_state";

/// One routed method call.
pub struct ObjectTarget {
    pub class: String,
    pub name: String,
    pub method: String,
    pub arguments: Vec<serde_json::Value>,
    /// Object keys already on this call's stack; the router refuses a
    /// target that appears here, because its mailbox is busy underneath
    /// this very call.
    pub chain: Vec<String>,
}

/// The call chain the currently dispatched method arrived on; app data in
/// pinned vms, absent in request vms. One slot suffices because a pinned
/// vm runs exactly one call at a time.
pub struct CallChain(pub Vec<String>);

/// What `__dispatch` receives from the mailbox, mirroring [`ObjectTarget`]
/// minus the name, which the pinned vm embodies rather than reads.
#[derive(Deserialize)]
struct DispatchCall {
    class: String,
    method: String,
    #[serde(default)]
    args: Vec<serde_json::Value>,
    /// The stack this call rides on, own key included, installed for the
    /// method's outbound calls to extend.
    #[serde(default)]
    chain: Vec<String>,
}

/// How a method call leaves this vm: the worker supplies the routing (id
/// derivation, placement, the mailbox); the vm only speaks this shape.
pub type RouterFuture =
    std::pin::Pin<Box<dyn Future<Output = Result<serde_json::Value, String>> + Send>>;
pub type ObjectRouter = Arc<dyn Fn(ObjectTarget) -> RouterFuture + Send + Sync>;

pub struct ObjectExtension;

impl LuaExtension for ObjectExtension {
    fn extension_info(&self) -> ExtensionInfo<'_> {
        ExtensionInfo {
            name: "object",
            description: "Durable object classes with serialized method calls",
            default: true,
        }
    }

    fn create_extension(&self, lua: &mlua::Lua) -> mlua::Result<mlua::Value> {
        lua.set_named_registry_value(CLASSES_KEY, lua.create_table()?)?;

        // `objects "Class"`: reference a class declared elsewhere. It mints
        // the same handle; whether the class exists is the callee's truth.
        lua.globals().set(
            "objects",
            lua.create_function(|lua, class: String| {
                ActiasRuntime::assert_declaration_phase(lua, "objects")?;
                class_handle(lua, class)
            })?,
        )?;

        install_dispatch(lua)?;

        // `object "Class" { methods }`, curried: the name is checked and
        // recorded, the table becomes the class body in this vm.
        let declaration = lua.create_function(|lua, class: String| {
            ActiasRuntime::assert_declaration_phase(lua, "object")?;
            ActiasRuntime::assert_contract_allows(lua, ContractKind::Object, &class)?;
            ActiasRuntime::record_object_declaration(lua, &class);

            lua.create_function(move |lua, methods: Table| {
                let classes: Table = lua.named_registry_value(CLASSES_KEY)?;
                classes.set(class.as_str(), methods)?;
                class_handle(lua, class.clone())
            })
        })?;

        Ok(mlua::Value::Function(declaration))
    }
}

/// The class handle: `Class:get(name)` mints an instance handle.
fn class_handle(lua: &Lua, class: String) -> mlua::Result<Table> {
    let handle = lua.create_table()?;
    handle.set("__class", class)?;

    handle.set(
        "get",
        lua.create_function(|lua, (this, name): (Table, String)| {
            let class: String = this.get("__class")?;
            instance_handle(lua, class, name)
        })?,
    )?;

    Ok(handle)
}

/// The instance handle: any method name resolves to a routed call.
fn instance_handle(lua: &Lua, class: String, name: String) -> mlua::Result<Table> {
    let handle = lua.create_table()?;
    handle.set("__class", class)?;
    handle.set("__name", name)?;

    let meta = lua.create_table()?;
    meta.set(
        "__index",
        lua.create_function(|lua, (this, method): (Table, String)| {
            let class: String = this.get("__class")?;
            let name: String = this.get("__name")?;

            lua.create_async_function(move |lua, args: mlua::MultiValue| {
                let class = class.clone();
                let name = name.clone();
                let method = method.clone();

                async move {
                    // The colon-call receiver is the handle itself; what
                    // travels is everything after it, as plain values.
                    let mut values = args.into_iter();
                    let _receiver = values.next();
                    let mut arguments = Vec::new();
                    for value in values {
                        arguments.push(lua.from_value::<serde_json::Value>(value)?);
                    }

                    let (router, chain) = {
                        let Some(router) = lua.app_data_ref::<ObjectRouter>() else {
                            return Err(mlua::Error::RuntimeError(
                                "Objects are not available in this runtime.".to_owned(),
                            ));
                        };
                        let chain = lua
                            .app_data_ref::<CallChain>()
                            .map(|chain| chain.0.clone())
                            .unwrap_or_default();
                        (router.clone(), chain)
                    };

                    let result = router(ObjectTarget {
                        class,
                        name,
                        method,
                        arguments,
                        chain,
                    })
                    .await
                    .map_err(mlua::Error::RuntimeError)?;

                    lua.to_value(&result)
                }
            })
        })?,
    )?;

    handle.set_metatable(Some(meta))?;
    Ok(handle)
}

/// Installs the receiving side: resolve the class method in this vm and
/// run it with the object's state table first.
fn install_dispatch(lua: &Lua) -> mlua::Result<()> {
    lua.globals().set(
        "__dispatch",
        lua.create_async_function(|lua, payload: mlua::Value| async move {
            let call: DispatchCall = lua.from_value(payload)?;

            // The method's own outbound calls extend this stack; installing
            // it per dispatch is safe because this vm runs one call at a
            // time by construction.
            lua.set_app_data(CallChain(call.chain.clone()));

            let classes: Table = lua.named_registry_value(CLASSES_KEY)?;
            let class: Table = classes.get(call.class.as_str()).map_err(|_| {
                mlua::Error::RuntimeError(format!("No object class '{}'.", call.class))
            })?;
            let method: mlua::Function = class.get(call.method.as_str()).map_err(|_| {
                mlua::Error::RuntimeError(format!(
                    "Object class '{}' has no method '{}'.",
                    call.class, call.method
                ))
            })?;

            // The state table is the object's in-memory identity; it lives
            // exactly as long as the pinned vm. Durable storage arrives
            // underneath it separately.
            let state: Table = match lua.named_registry_value(STATE_KEY) {
                Ok(state) => state,
                Err(_) => {
                    let state = lua.create_table()?;
                    lua.set_named_registry_value(STATE_KEY, state.clone())?;
                    state
                }
            };

            let mut multi = mlua::MultiValue::new();
            multi.push_back(mlua::Value::Table(state));
            for argument in call.args {
                multi.push_back(lua.to_value(&argument)?);
            }

            method.call_async::<mlua::Value>(multi).await
        })?,
    )?;

    Ok(())
}
