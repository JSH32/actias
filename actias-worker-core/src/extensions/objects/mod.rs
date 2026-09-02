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

mod directory;
mod dispatch;
mod handles;
mod products;
mod registry;
mod state;

use mlua::Table;

use crate::runtime::extension::{ExtensionInfo, LuaExtension};
use crate::runtime::{ActiasRuntime, ContractKind};

/// The built-in class names live in [`actias_common::classes`], because
/// object identities cross service boundaries; re-exported here where the
/// runtime consumes them. The router special-cases [`DATABASE_CLASS`]'s
/// read methods for the mailbox bypass.
pub use actias_common::classes::{CRON_CLASS, DATABASE_CLASS, QUEUE_CLASS, WORKFLOW_CLASS};

/// The clocks and duration spellings live in [`crate::platform::time`];
/// re-exported here because the worker and cli crates reach them
/// through this module's path.
pub use crate::platform::time::{
    advance_clock_for_tests, cron_delay_ms, parse_duration_ms, unix_now_ms,
};
pub use directory::{DirectoryAnswer, DirectoryListFuture, DirectoryLister, DirectoryRequest};
/// Crate-internal: scratch evaluation needs the class's ladder, and the
/// registry that knows it is private to this module.
pub(crate) use dispatch::class_migrations;
pub use dispatch::{
    CallChain, CallerIdentity, ObjectRouter, ObjectTarget, PendingAlarm, RESERVED_METHODS,
    RouterFuture,
};
pub(crate) use dispatch::{CurrentDispatch, derive_directory};
pub(crate) use products::workflow_definition_handle;

/// Runs the class's admission gate for a fresh identity: [`Some`] with
/// the gate's verdict, [`None`] when the class declares none. The gate
/// takes only the name; it runs before storage exists.
///
/// # Errors
/// Returns the gate's own error text; a throwing gate refuses.
pub async fn admit(lua: &mlua::Lua, class: &str, name: &str) -> Result<Option<bool>, String> {
    let Some(table) = registry::ClassRegistry::of(lua).class_table(class) else {
        return Ok(None);
    };
    let gate: mlua::Value = table.get("admit").map_err(|e| e.to_string())?;
    let mlua::Value::Function(gate) = gate else {
        return Ok(None);
    };
    let verdict: bool = gate
        .call_async(name.to_owned())
        .await
        .map_err(|e| e.to_string())?;
    Ok(Some(verdict))
}

use handles::class_handle;

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
        // User classes only; platform classes dispatch in rust and never
        // appear here.
        registry::ClassRegistry::of(lua).install()?;
        products::install_products(lua)?;
        // The field kit a class writes its directory fields with. Data,
        // not a verb: the markers ARE the declaration, and the same
        // reader walks them at publish and at derivation.
        actias_declarations::field_kit::install(lua)?;

        dispatch::install_dispatch(lua)?;

        // `object "Class" { methods }`, curried: the name is checked and
        // recorded, the table becomes the class body in this vm.
        let declaration = lua.create_function(|lua, class: String| {
            ActiasRuntime::assert_declaration_phase(lua, "object")?;
            if class.starts_with("__") {
                return Err(mlua::Error::RuntimeError(format!(
                    "Class name '{class}' is reserved for the platform."
                )));
            }
            ActiasRuntime::assert_contract_allows(lua, ContractKind::Object, &class)?;
            ActiasRuntime::record_object_declaration(lua, &class);

            lua.create_function(move |lua, methods: Table| {
                // One parse normalizes the body and carries the
                // validation (the dead hooks.receive spelling refuses
                // in parity with extraction); the contract records
                // straight from the spec, the table keeps the
                // functions.
                let spec = registry::ClassSpec::parse(&class, &methods)?;
                if let Some(dir) = &spec.migrations {
                    ActiasRuntime::record_object_migrations(lua, &class, dir);
                }
                for declared in &spec.declared_publishes {
                    ActiasRuntime::record_publishes_declaration(lua, format!("{class}:{declared}"));
                }
                registry::ClassRegistry::of(lua).declare(spec, &methods)?;
                class_handle(lua, class.clone())
            })
        })?;

        Ok(mlua::Value::Function(declaration))
    }
}
