//! The class registry: declared class bodies normalize into the shared
//! [`ClassSpec`], the same reader the extraction pass runs (see
//! actias-declarations), so the stored contract and this enforcing
//! runtime agree by construction. The registry view is one door for
//! both halves: the lua side (class body tables in the named registry,
//! home of the functions) and the rust side (the typed specs in app
//! data). Declaration is a single operation so the halves cannot
//! drift.

use std::collections::HashMap;
use std::sync::Arc;

use mlua::{Lua, Table};

pub(crate) use actias_declarations::{ClassSpec, TopicPolicy};

/// Registry key of the class-name-to-methods table in this vm; the
/// registry view below is the only door, so no sibling reads the slot
/// raw.
const CLASSES_KEY: &str = "object_classes";

/// The vm's specs by class name, app data beside the lua registry.
struct ClassSpecs(HashMap<String, Arc<ClassSpec>>);

/// The vm's class registry.
pub(super) struct ClassRegistry<'lua> {
    lua: &'lua Lua,
}

impl<'lua> ClassRegistry<'lua> {
    /// The registry of the vm behind `lua`.
    pub(super) fn of(lua: &'lua Lua) -> Self {
        Self { lua }
    }

    /// Installs the empty class table at extension boot.
    pub(super) fn install(&self) -> mlua::Result<()> {
        let classes = self.lua.create_table()?;
        self.lua.set_named_registry_value(CLASSES_KEY, classes)
    }

    /// Declares a class: the body table becomes reachable under its
    /// name and its spec is recorded, together.
    pub(super) fn declare(&self, spec: ClassSpec, body: &Table) -> mlua::Result<()> {
        let classes: Table = self.lua.named_registry_value(CLASSES_KEY)?;
        classes.set(spec.name.as_str(), body)?;
        if self.lua.app_data_ref::<ClassSpecs>().is_none() {
            self.lua.set_app_data(ClassSpecs(HashMap::new()));
        }
        if let Some(mut specs) = self.lua.app_data_mut::<ClassSpecs>() {
            specs.0.insert(spec.name.clone(), Arc::new(spec));
        }
        Ok(())
    }

    /// The spec a declaration stored, or [`None`] for a class this vm
    /// never declared (platform classes never appear here).
    pub(super) fn spec(&self, class: &str) -> Option<Arc<ClassSpec>> {
        self.lua
            .app_data_ref::<ClassSpecs>()
            .and_then(|specs| specs.0.get(class).cloned())
    }

    /// One class's body table, the home of its functions; [`None`] for
    /// a class this vm never declared.
    pub(super) fn class_table(&self, class: &str) -> Option<Table> {
        self.lua
            .named_registry_value::<Table>(CLASSES_KEY)
            .ok()?
            .get(class)
            .ok()
    }
}
