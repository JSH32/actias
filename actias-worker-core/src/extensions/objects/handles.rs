//! The handle substrate: class handles that mint instance handles, and
//! instance handles whose method access routes one mailbox call. The
//! products built on top live in `products.rs`.

use mlua::{Lua, LuaSerdeExt, Table};

use super::dispatch::{CallChain, ObjectRouter, ObjectTarget, callable_method};

/// The class handle: `Class(name)` (or the long `Class:get(name)`) mints
/// an instance handle.
pub(super) fn class_handle(lua: &Lua, class: String) -> mlua::Result<Table> {
    let handle = lua.create_table()?;
    handle.set("__class", class)?;

    handle.set(
        "get",
        lua.create_function(|lua, (this, name): (Table, String)| {
            let class: String = this.get("__class")?;
            instance_handle(lua, class, name)
        })?,
    )?;

    // Callable: `Channel(id)` is the blessed spelling of `:get(id)`.
    let meta = lua.create_table()?;
    meta.set(
        "__call",
        lua.create_function(|lua, (this, name): (Table, String)| {
            let class: String = this.get("__class")?;
            instance_handle(lua, class, name)
        })?,
    )?;
    handle.set_metatable(Some(meta))?;

    Ok(handle)
}

/// The instance handle: any method name resolves to a routed call.
pub(super) fn instance_handle(lua: &Lua, class: String, name: String) -> mlua::Result<Table> {
    let handle = lua.create_table()?;
    handle.set("__class", class)?;
    handle.set("__name", name)?;

    let meta = lua.create_table()?;
    meta.set(
        "__index",
        lua.create_function(|lua, (this, method): (Table, String)| {
            let class: String = this.get("__class")?;
            let name: String = this.get("__name")?;
            if !callable_method(&method) {
                return Err(mlua::Error::RuntimeError(format!(
                    "'{method}' is a platform hook; only the platform calls it."
                )));
            }

            lua.create_async_function(move |lua, args: mlua::MultiValue| {
                let class = class.clone();
                let name = name.clone();
                let method = method.clone();

                async move {
                    // In workflow vms, effects live inside steps alone;
                    // everywhere else this is a no-op.
                    crate::platform::workflow::assert_effects_allowed(&lua)?;
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
                        // The router knows whose vm this is; it fills this.
                        caller: None,
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
