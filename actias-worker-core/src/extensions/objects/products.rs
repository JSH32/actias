//! The product sugar, each a declaration over the object substrate:
//! `database` (a body may name its migrations), `queue`, `workflows`
//! with its start/get definition handle, and `objects`, the reference
//! form for a class declared elsewhere.

use mlua::{Lua, LuaSerdeExt, Table};

use super::handles::{class_handle, instance_handle};
use super::{DATABASE_CLASS, QUEUE_CLASS, WORKFLOW_CLASS};
use crate::runtime::{ActiasRuntime, ContractKind};

/// The workflow definition handle `workflow "name" (fn)` returns and
/// `workflows "name"` looks up: `start` mints a run and kicks its first
/// attempt, `get` addresses an existing run. Run handles are ordinary
/// instance handles on the workflow class, so signal/cancel/status route
/// like any object method.
pub(crate) fn workflow_definition_handle(lua: &Lua, definition: String) -> mlua::Result<Table> {
    let handle = lua.create_table()?;
    handle.set("__definition", definition)?;

    let meta = lua.create_table()?;
    meta.set(
        "__index",
        lua.create_function(|lua, (this, method): (Table, String)| {
            let definition: String = this.get("__definition")?;
            match method.as_str() {
                "start" => lua.create_async_function(move |lua, args: mlua::MultiValue| {
                    let definition = definition.clone();
                    async move {
                        let mut values = args.into_iter();
                        let _receiver = values.next();
                        let input = values
                            .next()
                            .map(|value| lua.from_value::<serde_json::Value>(value))
                            .transpose()?
                            .unwrap_or(serde_json::Value::Null);
                        let id = values
                            .next()
                            .and_then(|value| match value {
                                mlua::Value::Table(opts) => {
                                    opts.get::<Option<String>>("id").ok().flatten()
                                }
                                _ => None,
                            })
                            .ok_or_else(|| {
                                mlua::Error::RuntimeError(
                                    "start takes the input and { id = \"...\" }; the id is \
                                     the run's identity, so a retried start joins it."
                                        .to_owned(),
                                )
                            })?;
                        if id.trim().is_empty() || id.contains('/') {
                            return Err(mlua::Error::RuntimeError(
                                "A run id is a non-empty string without '/'.".to_owned(),
                            ));
                        }

                        let name = format!("{definition}/{id}");
                        let run = instance_handle(&lua, WORKFLOW_CLASS.to_owned(), name.clone())?;
                        let start: mlua::Function = run.get("start")?;
                        let outcome: mlua::Value = start
                            .call_async((run.clone(), lua.to_value(&input)?))
                            .await?;
                        run.set("started", outcome)?;
                        Ok(run)
                    }
                }),
                "get" => lua.create_function(move |lua, (_this, id): (Table, String)| {
                    instance_handle(lua, WORKFLOW_CLASS.to_owned(), format!("{definition}/{id}"))
                }),
                other => Err(mlua::Error::RuntimeError(format!(
                    "A workflow definition handle has start and get, not '{other}'."
                ))),
            }
        })?,
    )?;
    handle.set_metatable(Some(meta))?;
    Ok(handle)
}

/// Installs the product globals, each sugar over the object substrate.
pub(super) fn install_products(lua: &Lua) -> mlua::Result<()> {
    // `database "name"`: the sql product face, sugar over an object of
    // the built-in class; same lease, same file, same mailbox.
    lua.globals().set(
        "database",
        lua.create_function(|lua, name: String| {
            ActiasRuntime::assert_declaration_phase(lua, "database")?;
            ActiasRuntime::assert_contract_allows(lua, ContractKind::Database, &name)?;
            // `database "name"` is the whole declaration; the handle
            // also accepts a body, `database "name" { migrations =
            // "dir" }`, which records where its schema comes from.
            let handle = instance_handle(lua, DATABASE_CLASS.to_owned(), name.clone())?;
            let declared = name.clone();
            let meta = handle
                .metatable()
                .ok_or_else(|| mlua::Error::RuntimeError("handle has no metatable.".to_owned()))?;
            meta.set(
                "__call",
                lua.create_function(move |lua, (this, body): (Table, Table)| {
                    ActiasRuntime::assert_declaration_phase(lua, "database")?;
                    let dir: Option<String> = body.get("migrations").ok();
                    ActiasRuntime::record_database_declaration(
                        lua,
                        &match dir {
                            Some(dir) => format!("{declared}={dir}"),
                            None => declared.clone(),
                        },
                    );
                    Ok(this)
                })?,
            )?;
            ActiasRuntime::record_database_declaration(lua, &name);
            Ok(handle)
        })?,
    )?;

    // `queue "name"`: a durable message queue, sugar over an object of
    // the built-in class. `:send` enqueues; the revision declaring
    // `on "queue:<name>"` consumes, driven by the object's alarm.
    lua.globals().set(
        "queue",
        lua.create_function(|lua, name: String| {
            ActiasRuntime::assert_declaration_phase(lua, "queue")?;
            ActiasRuntime::assert_contract_allows(lua, ContractKind::Queue, &name)?;
            ActiasRuntime::record_queue_declaration(lua, &name);
            instance_handle(lua, QUEUE_CLASS.to_owned(), name)
        })?,
    )?;

    // `workflows "name"`: the definition handle, same one the
    // declaration returns; cross-script callers start and address
    // runs through it.
    lua.globals().set(
        "workflows",
        lua.create_function(|lua, name: String| {
            ActiasRuntime::assert_declaration_phase(lua, "workflows")?;
            workflow_definition_handle(lua, name)
        })?,
    )?;

    // `objects "Class"`: reference a class declared elsewhere. It mints
    // the same handle; whether the class exists is the callee's truth.
    // Platform classes are not addressable this way.
    lua.globals().set(
        "objects",
        lua.create_function(|lua, class: String| {
            ActiasRuntime::assert_declaration_phase(lua, "objects")?;
            if class.starts_with("__") {
                return Err(mlua::Error::RuntimeError(format!(
                    "Class name '{class}' is reserved for the platform."
                )));
            }
            class_handle(lua, class)
        })?,
    )?;
    Ok(())
}
