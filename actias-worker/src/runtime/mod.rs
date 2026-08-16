pub mod extension;

use crate::{
    extensions::{crypto::CryptoExtension, jwt::JwtExtension, kv::KvExtension},
    proto::{
        bundle::{Bundle, File},
        kv_service::kv_service_client::KvServiceClient,
        script_service::{Revision, Script},
    },
    runtime::extension::standard_extensions::{JsonExtension, UuidExtension},
};

use self::extension::LuaExtension;
use actias_common::tracing::trace;
use mlua::{AsChunk, ExternalResult, Lua, LuaSerdeExt, Table, UserData};
use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
    ops::Deref,
    sync::{Arc, RwLock},
    time::Instant,
};

/// Lua runtime with actias specific methods.
pub struct ActiasRuntime {
    lua: Lua,
    // Arc to pass to interrupt handler.
    timer: Arc<RwLock<Timer>>,
}

#[derive(Clone)]
struct Timer {
    start_time: Option<Instant>,
    time_limit: Option<u64>,
}

impl Deref for ActiasRuntime {
    type Target = Lua;

    fn deref(&self) -> &Self::Target {
        &self.lua
    }
}

/// Script info table exposed to lua.
#[derive(Serialize, Deserialize, Clone, Debug)]
struct ScriptInfo {
    /// Public script identifier
    identifier: String,
    project_id: String,
}

impl UserData for ScriptInfo {}

/// Canonical key for a module, so that a `require` argument and the bundle file
/// it resolves to always produce the same string.
///
/// Lua module syntax and paths both normalise to one form, with or without the
/// extension: `dir/mod.lua`, `dir/mod` and `dir.mod` all key to `dir.mod`.
fn module_key(name: &str) -> String {
    let name = name.trim_start_matches("./");
    let name = name.strip_suffix(".lua").unwrap_or(name);
    name.replace('/', ".")
}

/// Error for a runtime whose [`Bundle`] app data is missing.
///
/// Only reachable if the runtime was built without one, which
/// [`ActiasRuntime::new`] does not allow.
fn no_bundle() -> mlua::Error {
    mlua::Error::RuntimeError("Runtime has no bundle loaded.".into())
}

impl ActiasRuntime {
    /// Registry key holding the table of modules loaded so far.
    const MODULE_REGISTRY: &'static str = "module_registry";

    /// Event fired for an inbound http request.
    pub const FETCH_EVENT: &'static str = "fetch";

    /// Events a script may register a listener for.
    const EVENTS: [&'static str; 1] = [Self::FETCH_EVENT];

    /// Registry key holding the listener registered for `event`.
    ///
    /// Both `add_event_listener` and [`ActiasRuntime::listener`] go through
    /// this, so the two cannot disagree about how a key is spelled.
    fn listener_key(event: &str) -> String {
        format!("listener_{event}")
    }

    /// Listener a script registered for `event`.
    ///
    /// # Errors
    /// Returns [`mlua::Error`] when the script registered no listener for it.
    pub fn listener(&self, event: &str) -> mlua::Result<mlua::Function<'_>> {
        self.lua.named_registry_value(&Self::listener_key(event))
    }

    /// Table of modules loaded so far, keyed by [`module_key`].
    ///
    /// # Errors
    /// Returns [`mlua::Error`] if the registry was never initialised by
    /// [`ActiasRuntime::set_module_loaders`].
    fn module_registry(lua: &Lua) -> mlua::Result<Table<'_>> {
        lua.named_registry_value(Self::MODULE_REGISTRY)
    }

    /// Installs `require`, `dofile` and `getfile` along with the registry they
    /// share, resolving everything against the [`Bundle`] in app data.
    ///
    /// Takes the [`Lua`] rather than `&self` because these globals need nothing
    /// but the bundle, which lets them be exercised without service clients.
    ///
    /// # Errors
    /// Returns [`mlua::Error`] if a global cannot be defined.
    fn set_module_loaders(lua: &Lua) -> mlua::Result<()> {
        trace!("Initializing module registry");
        lua.set_named_registry_value(Self::MODULE_REGISTRY, lua.create_table()?)?;

        // Global function to retrieve modules from the registry.
        lua.globals().set(
            "require",
            lua.create_async_function(|lua, module_name: String| async move {
                let key = module_key(&module_name);

                // A module body runs once per runtime; every later require of the
                // same module answers from the registry.
                let registry = Self::module_registry(lua)?;
                let cached: mlua::Value = registry.get(key.as_str())?;
                if !cached.is_nil() {
                    return Ok(cached);
                }

                // The bundle borrow ends before loading, so a module that itself
                // calls require does not hold it across the await.
                let module = {
                    let bundle = lua.app_data_ref::<Bundle>().ok_or_else(no_bundle)?;

                    match bundle
                        .files
                        .iter()
                        .find(|file| module_key(&file.file_path) == key)
                    {
                        Some(file) => LuaModule::from_file(file)?,
                        None => return Ok(mlua::Value::Nil),
                    }
                };

                let result: mlua::Value = lua.load(&module).eval_async().await?;
                registry.set(key, result.clone())?;

                Ok(result)
            })?,
        )?;

        // Like require but doesn't put anything in the module registry.
        lua.globals().set(
            "dofile",
            lua.create_async_function(|lua, module_name: String| async move {
                let module = {
                    let bundle = lua.app_data_ref::<Bundle>().ok_or_else(no_bundle)?;

                    match bundle
                        .files
                        .iter()
                        .find(|file| file.file_path == module_name)
                    {
                        Some(file) => LuaModule::from_file(file)?,
                        None => return Ok(mlua::Value::Nil),
                    }
                };

                lua.load(&module).eval_async().await
            })?,
        )?;

        lua.globals().set(
            "getfile",
            lua.create_function(|lua, path: String| {
                let bundle = lua.app_data_ref::<Bundle>().ok_or_else(no_bundle)?;
                let file = bundle.files.iter().find(|file| file.file_path == path);

                Ok(match file {
                    Some(v) => lua.to_value(&v.content)?,
                    None => mlua::Value::Nil,
                })
            })?,
        )?;

        Ok(())
    }

    /// Create a new [`ActiasRuntime`], this will run the main script from the entrypoint defined in the [`Bundle`].
    ///
    /// # Arguments
    /// - `script` - Script information, this is so the script can identify it's own routing pattern.
    /// - `revision` - Script revision, ensure that this revision has a [`Bundle`] (use `with_bundle`).
    /// - `kv_client` - Key value service client, allows the script to access/store persistent data.
    /// - `time_limit` - Total Time limit in seconds, this is based on seconds and will start when [`start_timer`] is called
    pub async fn new(
        script: Script,
        revision: Revision,
        kv_client: KvServiceClient<tonic::transport::Channel>,
        time_limit: Option<u64>,
    ) -> mlua::Result<Self> {
        trace!("Initializing lua runtime");

        let lua = Self {
            lua: Lua::new_with(
                mlua::StdLib::ALL_SAFE,
                mlua::LuaOptions::new().catch_rust_panics(false),
            )?,
            timer: Arc::new(RwLock::new(Timer {
                start_time: None,
                time_limit,
            })),
        };

        let bundle = revision.bundle.ok_or_else(|| {
            mlua::Error::RuntimeError("Revision was fetched without its bundle.".into())
        })?;
        lua.set_app_data::<Bundle>(bundle.clone());

        lua.sandbox(true)?;

        // 128 MB memory limit.
        lua.set_memory_limit(128 * 1000000)?;

        let timer = lua.timer.clone();

        // Time limit each worker total runtime.
        // TODO: Figure out how to make this CPU time based.
        // Next best thing is setting a hook trigger for every nth instruction.
        // With https://docs.rs/mlua/latest/mlua/struct.HookTriggers.html#structfield.every_nth_instruction
        // Or https://docs.rs/mlua/latest/mlua/struct.Lua.html#method.set_hook
        lua.set_interrupt(move |_| {
            let timer = timer.read().unwrap();
            if let (Some(start_time), Some(time_limit)) = (timer.start_time, timer.time_limit)
                && Instant::now().duration_since(start_time).as_secs() > time_limit
            {
                return Err(mlua::Error::RuntimeError(format!(
                    "Script timed out, limit is {} seconds.",
                    time_limit
                )));
            }

            Ok(mlua::VmState::Continue)
        });

        // Function to add listener to registry
        // All added listeners are prefixed with `_listener`
        lua.globals().set(
            "add_event_listener",
            lua.create_function(|lua, (event, callback): (String, mlua::Function)| {
                if !Self::EVENTS.contains(&event.as_str()) {
                    Err(mlua::Error::RuntimeError(format!(
                        "Invalid event '{event}', expected one of: {}.",
                        Self::EVENTS.join(", ")
                    )))
                } else {
                    lua.set_named_registry_value(&Self::listener_key(&event), callback)?;
                    Ok(())
                }
            })?,
        )?;

        Self::set_module_loaders(&lua)?;

        lua.register_extensions(&[
            &JsonExtension,
            &UuidExtension,
            &crate::extensions::http::HttpExtension,
            &KvExtension {
                kv_client,
                project_id: script.project_id.clone(),
            },
            &JwtExtension,
            &CryptoExtension,
        ])?;

        lua.globals().set(
            "script",
            lua.to_value(&ScriptInfo {
                identifier: script.public_identifier,
                project_id: script.project_id,
            })?,
        )?;

        let entry_point = bundle
            .files
            .iter()
            .find(|file| file.file_name == bundle.entry_point);

        // We need to set a new timer temporarily when registering.
        // This should be one second since nothing should be happening in this time.
        let original_timer = lua.timer.read().unwrap().clone();
        *lua.timer.write().unwrap() = Timer {
            start_time: Some(Instant::now()),
            time_limit: Some(1),
        };

        // Run entry point and register handlers.
        if let Some(entry_point) = entry_point {
            let _: () = lua
                .load(&LuaModule::from_file(entry_point)?)
                .eval_async()
                .await?;
        }

        // Set original timer.
        *lua.timer.write().unwrap() = original_timer;

        Ok(lua)
    }

    /// Start the timer. This only works if `time_limit` is set.
    /// This will stop the runtime with an error once the time limit has passed since the timer has been started.
    pub fn start_timer(&self) {
        let mut timer = self.timer.write().unwrap();
        if timer.time_limit.is_some() {
            timer.start_time = Some(Instant::now());
        }
    }

    /// Register an extension into the runtime.
    pub fn register_extensions(&self, extensions: &[&dyn LuaExtension]) -> mlua::Result<()> {
        for extension in extensions {
            let info = extension.extension_info();

            trace!(
                name = info.name,
                description = info.description,
                "Registering extension"
            );

            let extension = extension.create_extension(self)?;
            self.set_module(info.name, extension.clone())?;

            // Register extension as a global
            if info.default {
                self.globals().set(info.name, extension)?;
            }
        }

        Ok(())
    }

    /// Set a module from an object, making it visible to `require`.
    ///
    /// # Errors
    /// Returns [`mlua::Error`] if the module registry cannot be reached.
    pub fn set_module(&self, key: &str, value: mlua::Value) -> mlua::Result<()> {
        // The registry table is a handle into lua, so writing through it is
        // enough; there is nothing to store back.
        Self::module_registry(&self.lua)?.set(module_key(key), value)?;

        trace!(module_name = key, "Registered to module registry");

        Ok(())
    }
}

/// Lua module.
pub struct LuaModule {
    /// Name or path of the module.
    pub name: String,
    /// Source code of the module.
    pub source: String,
}

impl LuaModule {
    /// Reads a bundle file into a loadable module, named by its path so lua
    /// tracebacks point at something a user can find.
    ///
    /// # Errors
    /// Returns [`mlua::Error`] when the file content is not valid utf-8.
    fn from_file(file: &File) -> mlua::Result<Self> {
        Ok(Self {
            name: file.file_path.clone(),
            source: std::str::from_utf8(&file.content)
                .into_lua_err()?
                .to_string(),
        })
    }
}

impl AsChunk<'_, '_> for &LuaModule {
    fn source(self) -> std::io::Result<Cow<'static, [u8]>> {
        Ok(Cow::Owned(self.source.as_bytes().to_vec()))
    }

    fn name(&self) -> Option<String> {
        Some(self.name.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_key_accepts_every_spelling_of_one_module() {
        // A require argument and the bundle path it resolves to have to agree,
        // otherwise a cached module is never found again.
        let from_path = module_key("dir/mod.lua");

        assert_eq!(from_path, "dir.mod");
        assert_eq!(module_key("dir/mod"), from_path);
        assert_eq!(module_key("dir.mod"), from_path);
        assert_eq!(module_key("./dir/mod.lua"), from_path);
    }

    #[test]
    fn module_key_keeps_distinct_modules_apart() {
        assert_ne!(module_key("dir/mod.lua"), module_key("other/mod.lua"));
        assert_ne!(module_key("mod.lua"), module_key("dir/mod.lua"));
    }

    #[test]
    fn module_key_handles_a_bare_name() {
        assert_eq!(module_key("main"), "main");
        assert_eq!(module_key("main.lua"), "main");
    }

    /// Builds a lua state carrying `files` as its bundle, with the module
    /// loaders installed and no service clients.
    fn runtime_with(files: Vec<File>) -> Lua {
        let lua = Lua::new();
        lua.set_app_data(Bundle {
            entry_point: "main.lua".to_owned(),
            files,
        });
        ActiasRuntime::set_module_loaders(&lua).expect("module loaders install");
        lua
    }

    fn lua_file(path: &str, source: &str) -> File {
        File {
            revision_id: String::new(),
            file_name: path.rsplit('/').next().unwrap_or(path).to_owned(),
            file_path: path.to_owned(),
            content: source.as_bytes().to_vec(),
        }
    }

    #[tokio::test]
    async fn require_runs_a_module_once_however_it_is_spelled() {
        // The module counts its own executions, so a re-run is visible in the
        // value require hands back.
        let lua = runtime_with(vec![lua_file(
            "lib/counter.lua",
            "runs = (runs or 0) + 1 return runs",
        )]);

        let mut results = Vec::new();
        for spelling in ["lib.counter", "lib/counter", "lib/counter.lua"] {
            let value: i64 = lua
                .load(format!("return require(\"{spelling}\")"))
                .eval_async()
                .await
                .unwrap_or_else(|e| panic!("require({spelling:?}) failed: {e}"));
            results.push(value);
        }

        assert_eq!(
            results,
            vec![1, 1, 1],
            "module body re-ran instead of being served from the registry"
        );
    }

    #[test]
    fn listener_key_is_derived_in_one_place() {
        // The registering side and the reading side must agree on the spelling,
        // which they only do by both calling this.
        assert_eq!(
            ActiasRuntime::listener_key(ActiasRuntime::FETCH_EVENT),
            "listener_fetch"
        );
    }

    #[tokio::test]
    async fn dofile_reruns_the_module_every_call() {
        // dofile is the deliberate opposite of require, and it also proves the
        // counter above can increment, so the require test is not vacuous.
        let lua = runtime_with(vec![lua_file(
            "lib/counter.lua",
            "runs = (runs or 0) + 1 return runs",
        )]);

        let first: i64 = lua
            .load("return dofile(\"lib/counter.lua\")")
            .eval_async()
            .await
            .unwrap();
        let second: i64 = lua
            .load("return dofile(\"lib/counter.lua\")")
            .eval_async()
            .await
            .unwrap();

        assert_eq!((first, second), (1, 2));
    }

    #[tokio::test]
    async fn require_of_an_absent_module_is_nil() {
        let lua = runtime_with(vec![]);

        let value: mlua::Value = lua
            .load("return require(\"nope\")")
            .eval_async()
            .await
            .unwrap();

        assert!(value.is_nil(), "expected nil, got {value:?}");
    }

    #[tokio::test]
    async fn module_loaders_error_rather_than_panic_without_a_bundle() {
        // app_data is absent here, which the loaders must report as an error.
        let lua = Lua::new();
        ActiasRuntime::set_module_loaders(&lua).unwrap();

        let result: mlua::Result<mlua::Value> =
            lua.load("return require(\"anything\")").eval_async().await;

        assert!(result.is_err(), "expected an error, got {result:?}");
    }
}
