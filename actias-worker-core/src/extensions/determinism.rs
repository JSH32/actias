//! The enforced-determinism surface for workflow vms.
//!
//! Workflow code must replay to the same result forever, so the ambient
//! nondeterminism our own extension set provides is either journaled or
//! refused here. Time and uuids WORK: each read consults a
//! [`Determinism`] source that records on first execution and replays
//! identically (W3 backs it with the journal cursor). Effects (http, kv,
//! objects, secrets, jwt, crypto) are refused by name with the one error
//! text that teaches the fix. Randomness is seeded once per instance in
//! the profile setup, not here.

use std::sync::Arc;

use crate::runtime::extension::{ExtensionInfo, LuaExtension};

/// The error every refused surface raises; the fix is in the text.
pub const FORBIDDEN: &str = "not available in workflow code, perform effects inside wf:step";

/// Where shimmed reads come from: recording on first execution,
/// replaying from the journal afterwards. One per instance, installed as
/// vm app data by whoever builds the workflow vm.
pub trait Determinism: Send + Sync {
    /// Unix seconds, journaled per read because time must advance.
    fn time(&self) -> i64;
    /// A v4-shaped id, journaled per call.
    fn uuid(&self) -> String;
    /// The per-instance random seed, recorded once in STARTED.
    fn seed(&self) -> i64;
}

/// The recorder handle the shims reach through vm app data.
#[derive(Clone)]
pub struct DeterminismSource(pub Arc<dyn Determinism>);

/// Reads the installed source or refuses: a workflow vm without one is a
/// wiring bug, and running nondeterministically anyway would corrupt the
/// journal's promise.
fn source(lua: &mlua::Lua) -> mlua::Result<DeterminismSource> {
    lua.app_data_ref::<DeterminismSource>()
        .map(|handle| handle.clone())
        .ok_or_else(|| {
            mlua::Error::RuntimeError(
                "This workflow vm has no determinism source; refusing to run.".to_owned(),
            )
        })
}

/// `uuid` for workflow vms: same name and shape as the standard
/// extension, every id journaled.
pub struct JournaledUuidExtension;

impl LuaExtension for JournaledUuidExtension {
    fn create_extension(&self, lua: &mlua::Lua) -> mlua::Result<mlua::Value> {
        let uuid = lua.create_table()?;
        uuid.set(
            "v4",
            lua.create_function(|lua, _: ()| Ok(source(lua)?.0.uuid()))?,
        )?;
        Ok(mlua::Value::Table(uuid))
    }

    fn extension_info(&self) -> ExtensionInfo<'_> {
        ExtensionInfo {
            name: "uuid",
            description: "UUIDs that record on first execution and replay identically.",
            default: true,
        }
    }
}

/// A name that exists only to refuse: any index or call raises
/// [`FORBIDDEN`], so the enforcement is visible at the exact boundary
/// the code crossed.
pub struct ForbiddenExtension {
    pub name: &'static str,
}

impl LuaExtension for ForbiddenExtension {
    fn create_extension(&self, lua: &mlua::Lua) -> mlua::Result<mlua::Value> {
        let surface = lua.create_table()?;
        let guard = lua.create_table()?;
        let refuse = lua.create_function(|_, _: mlua::MultiValue| {
            Err::<(), _>(mlua::Error::RuntimeError(FORBIDDEN.to_owned()))
        })?;
        guard.set("__index", refuse.clone())?;
        guard.set("__call", refuse)?;
        surface.set_metatable(Some(guard))?;
        Ok(mlua::Value::Table(surface))
    }

    fn extension_info(&self) -> ExtensionInfo<'_> {
        ExtensionInfo {
            name: self.name,
            description: "Refused in workflow code; effects belong inside wf:step.",
            default: true,
        }
    }
}

/// Installs the deterministic stdlib overrides: `os.time`/`os.clock`
/// consult the source, `math.randomseed` applies the instance seed once
/// and further re-seeding is refused (a code-chosen seed would replay,
/// but a time-derived one is the classic leak).
///
/// # Errors
/// Returns [`mlua::Error`] when the globals cannot be written.
pub fn shim_stdlib(lua: &mlua::Lua, seed: i64) -> mlua::Result<()> {
    let globals = lua.globals();

    // Luau's sandbox marks stdlib tables readonly; the patch window
    // opens and closes around exactly these writes.
    let os: mlua::Table = globals.get("os")?;
    os.set_readonly(false);
    os.set(
        "time",
        lua.create_function(|lua, _: mlua::MultiValue| Ok(source(lua)?.0.time()))?,
    )?;
    os.set(
        "clock",
        lua.create_function(|lua, _: ()| Ok(source(lua)?.0.time() as f64))?,
    )?;
    os.set(
        "date",
        lua.create_function(|_, _: mlua::MultiValue| {
            Err::<(), _>(mlua::Error::RuntimeError(FORBIDDEN.to_owned()))
        })?,
    )?;

    os.set_readonly(true);

    let math: mlua::Table = globals.get("math")?;
    math.set_readonly(false);
    let native_seed: mlua::Function = math.get("randomseed")?;
    native_seed.call::<()>(seed)?;
    math.set(
        "randomseed",
        lua.create_function(|_, _: mlua::MultiValue| {
            Err::<(), _>(mlua::Error::RuntimeError(FORBIDDEN.to_owned()))
        })?,
    )?;
    math.set_readonly(true);

    Ok(())
}
