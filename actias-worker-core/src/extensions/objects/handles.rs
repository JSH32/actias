//! The handle substrate: class handles that mint instance handles, and
//! instance handles whose method access routes one mailbox call. The
//! products built on top live in `products.rs`.

use mlua::{Lua, LuaSerdeExt, Table};

use super::dispatch::{CallChain, ObjectRouter, ObjectTarget, callable_method};

/// Passes a name a caller chose or is holding, or refuses it with the
/// rule it broke (an empty name): the one spelling lives in
/// [`actias_common::naming`].
fn checked_name(name: String) -> mlua::Result<String> {
    actias_common::naming::validate_name(&name)
        .map(|()| name)
        .map_err(|error| mlua::Error::RuntimeError(error.to_string()))
}

/// Answers one directory request through the runtime's lister seam.
///
/// # Errors
/// A runtime with no route to the directory (a vm the worker never
/// installed one in), or the read's own refusal: an unknown field, a
/// field still building, the store's message.
async fn answer(lua: &Lua, request: super::directory::DirectoryRequest) -> mlua::Result<Table> {
    let lister = lua
        .app_data_ref::<super::directory::DirectoryLister>()
        .ok_or_else(|| {
            mlua::Error::RuntimeError(
                "Reading a class's directory needs a route to it, which this runtime has none."
                    .to_owned(),
            )
        })?
        .clone();
    let answered = lister(request).await.map_err(mlua::Error::RuntimeError)?;
    super::directory::answer_to_lua(lua, answered)
}

/// The class handle: `Class(name)` (or the long `Class:get(name)`) mints
/// an instance handle.
pub(super) fn class_handle(lua: &Lua, class: String) -> mlua::Result<Table> {
    let handle = lua.create_table()?;
    handle.set("__class", class)?;

    handle.set(
        "get",
        lua.create_function(|lua, (this, name): (Table, String)| {
            let class: String = this.get("__class")?;
            instance_handle(lua, class, checked_name(name)?)
        })?,
    )?;

    // `Class:list { ... }`, `Class:find { ... }` and `Class:visit
    // { ... }`: the directory, not an instance. Async because the
    // answer may need files this node has not cached; no object is
    // woken and no lease is taken by any of them.
    handle.set(
        "list",
        lua.create_async_function(|lua, (this, options): (Table, Option<Table>)| async move {
            let class: String = this.get("__class")?;
            answer(
                &lua,
                super::directory::parse_request(class, options, false)?,
            )
            .await
        })?,
    )?;

    // The shorthand: the argument IS the predicate, and order and limit
    // take their defaults.
    handle.set(
        "find",
        lua.create_async_function(
            |lua, (this, predicate): (Table, Option<Table>)| async move {
                let class: String = this.get("__class")?;
                answer(&lua, super::directory::parse_find(class, predicate)?).await
            },
        )?,
    )?;

    // The verified read: every candidate is checked against its own
    // object's shipping manifest, so a row that stopped matching is
    // dropped and a fresher one is served fresh. A row that cannot be
    // checked is served carrying `unverified`, never dropped.
    handle.set(
        "visit",
        lua.create_async_function(|lua, (this, options): (Table, Option<Table>)| async move {
            let class: String = this.get("__class")?;
            answer(&lua, super::directory::parse_request(class, options, true)?).await
        })?,
    )?;

    // Callable: `Channel(id)` is the blessed spelling of `:get(id)`.
    let meta = lua.create_table()?;
    meta.set(
        "__call",
        lua.create_function(|lua, (this, name): (Table, String)| {
            let class: String = this.get("__class")?;
            instance_handle(lua, class, checked_name(name)?)
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
                    // A read-only shell session refuses every method call
                    // except a database's reading verbs: a method on a
                    // user object runs code with effects, and exec writes.
                    let database_read = class == actias_common::classes::DATABASE_CLASS
                        && matches!(method.as_str(), "query" | "query_one" | "read" | "read_one");
                    if !database_read {
                        crate::runtime::ActiasRuntime::assert_writes_allowed(
                            &lua,
                            &format!("{class}:{method}"),
                        )?;
                    }
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

                    // A json null in a result is Lua `nil`, never mlua's
                    // null sentinel, for the same reason dispatch treats
                    // arguments this way: the sentinel is truthy, so a
                    // missing `query_one` row would arrive as userdata
                    // and `row == nil` would silently miss it.
                    lua.to_value_with(
                        &result,
                        mlua::SerializeOptions::new()
                            .serialize_none_to_null(false)
                            .serialize_unit_to_null(false),
                    )
                }
            })
        })?,
    )?;

    handle.set_metatable(Some(meta))?;
    Ok(handle)
}

#[cfg(test)]
mod tests {
    const ROOM: &str = r#"
        Room = object "Room" {
            join = function(state, who) state.store:set("last", who) end,
        }
        on "fetch" (function() return { body = "ok" } end)
    "#;

    /// An empty name is refused where a user types it, naming the rule,
    /// in both spellings of a call.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_empty_name_is_refused() {
        let runtime = crate::objects::testing::runtime_with(ROOM).await;
        let error = runtime
            .load(r#"return Room:get("")"#)
            .eval_async::<mlua::Value>()
            .await
            .expect_err("refused");
        assert!(error.to_string().contains("non-empty"), "{error}");
        let error = runtime
            .load(r#"return Room("")"#)
            .eval_async::<mlua::Value>()
            .await
            .expect_err("refused");
        assert!(error.to_string().contains("non-empty"), "{error}");
    }
}
