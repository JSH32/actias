pub mod extension;

use crate::{
    extensions::{crypto::CryptoExtension, jwt::JwtExtension, kv::KvExtension},
    proto::{
        bundle::{Bundle, File, FileKind},
        kv_service::kv_service_client::KvServiceClient,
        script_service::{Revision, Script},
    },
    runtime::extension::standard_extensions::{JsonExtension, UuidExtension},
};

use self::extension::LuaExtension;
use actias_common::tracing::trace;
use mlua::{AsChunk, ChunkMode, ExternalResult, Lua, LuaSerdeExt, Table, UserData};
use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
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

/// Error for a runtime whose [`PreparedRevision`] app data is missing.
///
/// Only reachable if the runtime was built without one, which
/// [`ActiasRuntime::new`] does not allow.
fn no_revision() -> mlua::Error {
    mlua::Error::RuntimeError("Runtime has no revision loaded.".into())
}

/// Whether the vm is evaluating the entry point's top level, the only time
/// declaration forms (`kv "name"`, `on "fetch"`) may run.
///
/// Keeping declarations out of handlers is what makes the code extractable
/// as a manifest: a capability that could be minted per request could not be
/// recorded at publish.
struct DeclarationPhase(bool);

/// Everything the entry point declared, recorded as the declarations run.
///
/// This is the code-derived capability contract: the same pass `actias
/// publish` performs to store it with a revision.
#[derive(Default, Debug, Clone)]
pub struct Declarations {
    /// Names handed to `kv "name"`.
    pub kv: Vec<String>,
    /// Events handed to `on "event"`.
    pub events: Vec<String>,
    /// Names handed to `secret "name"`.
    pub secrets: Vec<String>,
    /// Classes handed to `object "Class"`.
    pub objects: Vec<String>,
}

/// The capability contract a revision was published with.
///
/// Extracted from the code by `actias publish`; when present, the vm only
/// honors declarations the contract records, so a bundle whose stored
/// contract disagrees with its code (tampering, a bypassed publish) fails
/// loudly instead of gaining capabilities.
pub struct Contract {
    kv: HashSet<String>,
    secrets: HashSet<String>,
    objects: HashSet<String>,
}

/// Which contract list a declaration checks against.
pub enum ContractKind {
    Kv,
    Secret,
    Object,
}

/// A revision compiled once and shared by every request that runs it.
///
/// Revisions are immutable, so the bundle and the luau bytecode of each lua
/// file are prepared a single time and cached; what remains per request is
/// creating a vm and running already-compiled chunks.
pub struct PreparedRevision {
    /// The script the revision belongs to, for identity and kv scoping.
    pub script: Script,
    /// Id of the revision this was prepared from; empty for live sessions.
    pub revision_id: String,
    bundle: Bundle,
    /// Compiled bytecode per lua file, keyed by exact file path.
    bytecode: HashMap<String, Arc<Vec<u8>>>,
    /// Present when the revision was published with a contract; live
    /// sessions and contract-less revisions stay unenforced.
    contract: Option<Contract>,
}

impl PreparedRevision {
    /// Compiles every lua file in `revision`'s bundle.
    ///
    /// A file that fails to compile is kept as source rather than failing the
    /// whole revision, so loading it later reports the syntax error exactly as
    /// an uncached load would, and only if the script actually reaches it.
    ///
    /// # Errors
    /// Returns [`mlua::Error`] when the revision carries no bundle.
    pub fn prepare(script: Script, revision: Revision) -> mlua::Result<Self> {
        let revision_id = revision.id.clone();
        let bundle = revision.bundle.ok_or_else(|| {
            mlua::Error::RuntimeError("Revision was fetched without its bundle.".into())
        })?;

        let compiler = mlua::Compiler::new();
        let mut bytecode = HashMap::new();

        for file in &bundle.files {
            if !file.file_path.ends_with(".lua") {
                continue;
            }

            match compiler.compile(&file.content) {
                Ok(code) => {
                    bytecode.insert(file.file_path.clone(), Arc::new(code));
                }
                Err(error) => trace!(
                    file = file.file_path,
                    %error,
                    "file kept as source, ahead-of-time compilation failed"
                ),
            }
        }

        let contract = revision
            .script_config
            .and_then(|config| config.capabilities)
            .map(|capabilities| Contract {
                kv: capabilities.kv.into_iter().collect(),
                secrets: capabilities.secrets.into_iter().collect(),
                objects: capabilities.objects.into_iter().collect(),
            });

        Ok(Self {
            script,
            revision_id,
            bundle,
            bytecode,
            contract,
        })
    }

    /// Bytes this revision occupies, for weighing cache entries.
    pub fn weight(&self) -> u64 {
        let source: usize = self.bundle.files.iter().map(|f| f.content.len()).sum();
        let compiled: usize = self.bytecode.values().map(|b| b.len()).sum();
        (source + compiled) as u64
    }

    /// File at exactly `path`, if the bundle has one.
    fn file(&self, path: &str) -> Option<&File> {
        self.bundle.files.iter().find(|file| file.file_path == path)
    }

    /// Asset served for `path`, if the bundle carries one.
    ///
    /// Only `kind: asset` files are eligible, so lua source is never handed
    /// out raw. A directory path (empty or trailing slash) falls back to its
    /// index.html, which is what makes a bundle of nothing but assets
    /// servable at its root.
    pub fn asset(&self, path: &str) -> Option<&File> {
        let find = |target: &str| {
            self.bundle
                .files
                .iter()
                .find(|file| file.kind == FileKind::Asset as i32 && file.file_path == target)
        };

        if let Some(file) = find(path) {
            return Some(file);
        }

        if path.is_empty() || path.ends_with('/') {
            return find(&format!("{path}index.html"));
        }

        None
    }

    /// Loadable module for the bundle file at exactly `path`.
    fn module_by_path(&self, path: &str) -> Option<mlua::Result<LuaModule>> {
        self.file(path).map(|file| self.module_for(file))
    }

    /// Loadable module for the file whose [`module_key`] matches `key`.
    fn module_by_key(&self, key: &str) -> Option<mlua::Result<LuaModule>> {
        self.bundle
            .files
            .iter()
            .find(|file| module_key(&file.file_path) == key)
            .map(|file| self.module_for(file))
    }

    /// Entry point module the bundle names, if it exists.
    fn entry_module(&self) -> Option<mlua::Result<LuaModule>> {
        self.module_by_path(&self.bundle.entry_point)
    }

    /// Wraps `file` as a loadable module, preferring its compiled bytecode.
    ///
    /// # Errors
    /// Returns [`mlua::Error`] when an uncompiled file is not valid utf-8.
    fn module_for(&self, file: &File) -> mlua::Result<LuaModule> {
        let code = match self.bytecode.get(&file.file_path) {
            Some(code) => ModuleCode::Bytecode(code.clone()),
            None => ModuleCode::Source(
                std::str::from_utf8(&file.content)
                    .into_lua_err()?
                    .to_string(),
            ),
        };

        Ok(LuaModule {
            name: file.file_path.clone(),
            code,
        })
    }
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
    pub fn listener(&self, event: &str) -> mlua::Result<mlua::Function> {
        self.lua.named_registry_value(&Self::listener_key(event))
    }

    /// Errors unless the vm is evaluating the entry point's top level.
    ///
    /// Every declaration form calls this first, so `kv "x"` inside a handler
    /// fails with the same message everywhere.
    pub fn assert_declaration_phase(lua: &Lua, form: &str) -> mlua::Result<()> {
        let declaring = lua
            .app_data_ref::<DeclarationPhase>()
            .map(|phase| phase.0)
            .unwrap_or(false);

        if declaring {
            Ok(())
        } else {
            Err(mlua::Error::RuntimeError(format!(
                "'{form}' is a declaration and is only available at the top level of the entry point"
            )))
        }
    }

    /// Errors when the revision's published [`Contract`] does not record
    /// `name` for `kind`; contract-less revisions pass.
    pub fn assert_contract_allows(lua: &Lua, kind: ContractKind, name: &str) -> mlua::Result<()> {
        let Some(prepared) = lua.app_data_ref::<Arc<PreparedRevision>>() else {
            return Ok(());
        };
        let Some(contract) = &prepared.contract else {
            return Ok(());
        };

        let (allowed, what) = match kind {
            ContractKind::Kv => (&contract.kv, "Namespace"),
            ContractKind::Secret => (&contract.secrets, "Secret"),
            ContractKind::Object => (&contract.objects, "Object class"),
        };

        if allowed.contains(name) {
            Ok(())
        } else {
            Err(mlua::Error::RuntimeError(format!(
                "{what} '{name}' is not in this revision's capability contract; republish after declaring it."
            )))
        }
    }

    /// Records one declaration into the vm's [`Declarations`].
    pub fn record_kv_declaration(lua: &Lua, namespace: &str) {
        if let Some(mut declarations) = lua.app_data_mut::<Declarations>() {
            declarations.kv.push(namespace.to_owned());
        }
    }

    /// Records one `secret` declaration into the vm's [`Declarations`].
    /// Notes an `object "Class"` declaration for [`Self::declarations`].
    pub fn record_object_declaration(lua: &Lua, class: &str) {
        if let Some(mut declarations) = lua.app_data_mut::<Declarations>() {
            declarations.objects.push(class.to_owned());
        }
    }

    pub fn record_secret_declaration(lua: &Lua, name: &str) {
        if let Some(mut declarations) = lua.app_data_mut::<Declarations>() {
            declarations.secrets.push(name.to_owned());
        }
    }

    /// Everything the entry point declared.
    ///
    /// Recording is load-bearing (the declaration forms write here); nothing
    /// at runtime reads it back yet, so the accessor is test-only until an
    /// enforcement or introspection consumer exists.
    #[cfg(test)]
    pub fn declarations(&self) -> Declarations {
        self.lua
            .app_data_ref::<Declarations>()
            .map(|d| d.clone())
            .unwrap_or_default()
    }

    /// Installs the `on` declaration: `on "fetch" (handler)` registers the
    /// handler for the event, replacing `add_event_listener`.
    ///
    /// Takes the [`Lua`] rather than `&self` for the same reason as
    /// [`ActiasRuntime::set_module_loaders`]: nothing here needs clients, so
    /// tests can exercise it without any.
    fn set_event_declaration(lua: &Lua) -> mlua::Result<()> {
        lua.globals().set(
            "on",
            lua.create_function(|lua, event: String| {
                Self::assert_declaration_phase(lua, "on")?;

                if !Self::EVENTS.contains(&event.as_str()) {
                    return Err(mlua::Error::RuntimeError(format!(
                        "Invalid event '{event}', expected one of: {}.",
                        Self::EVENTS.join(", ")
                    )));
                }

                if let Some(mut declarations) = lua.app_data_mut::<Declarations>() {
                    declarations.events.push(event.clone());
                }

                // `on "fetch" (fn)` is `on("fetch")(fn)`, so the declaration
                // returns the registrar that takes the handler.
                lua.create_function(move |lua, callback: mlua::Function| {
                    lua.set_named_registry_value(&Self::listener_key(&event), callback)
                })
            })?,
        )
    }

    /// Table of modules loaded so far, keyed by [`module_key`].
    ///
    /// # Errors
    /// Returns [`mlua::Error`] if the registry was never initialised by
    /// [`ActiasRuntime::set_module_loaders`].
    fn module_registry(lua: &Lua) -> mlua::Result<Table> {
        lua.named_registry_value(Self::MODULE_REGISTRY)
    }

    /// Installs `require`, `dofile` and `getfile` along with the registry they
    /// share, resolving everything against the [`PreparedRevision`] in app data.
    ///
    /// Takes the [`Lua`] rather than `&self` because these globals need nothing
    /// but the revision, which lets them be exercised without service clients.
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
                let registry = Self::module_registry(&lua)?;
                let cached: mlua::Value = registry.get(key.as_str())?;
                if !cached.is_nil() {
                    return Ok(cached);
                }

                // The revision borrow ends before loading, so a module that
                // itself calls require does not hold it across the await.
                let module = {
                    let prepared = lua
                        .app_data_ref::<Arc<PreparedRevision>>()
                        .ok_or_else(no_revision)?;

                    match prepared.module_by_key(&key) {
                        Some(module) => module?,
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
                    let prepared = lua
                        .app_data_ref::<Arc<PreparedRevision>>()
                        .ok_or_else(no_revision)?;

                    match prepared.module_by_path(&module_name) {
                        Some(module) => module?,
                        None => return Ok(mlua::Value::Nil),
                    }
                };

                lua.load(&module).eval_async().await
            })?,
        )?;

        lua.globals().set(
            "getfile",
            lua.create_function(|lua, path: String| {
                let prepared = lua
                    .app_data_ref::<Arc<PreparedRevision>>()
                    .ok_or_else(no_revision)?;

                Ok(match prepared.file(&path) {
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
    /// - `prepared` - Compiled revision, shared with the cache; the vm loads its bytecode without recompiling.
    /// - `kv_client` - Key value service client, allows the script to access/store persistent data.
    /// - `egress` - Guarded http client for the script's outbound requests.
    /// - `logs` - Where the script's log output goes; [`None`] keeps it in worker tracing only.
    /// - `secrets_key` - Key decrypting stored secrets; [`None`] makes `secret` declarations error.
    /// - `time_limit` - Total Time limit in seconds, this is based on seconds and will start when [`start_timer`] is called
    pub async fn new(
        prepared: Arc<PreparedRevision>,
        kv_client: KvServiceClient<tonic::transport::Channel>,
        egress: crate::egress::EgressClient,
        logs: Option<crate::extensions::log::LogPublisher>,
        secrets_key: Option<Arc<[u8; crate::extensions::secrets::KEY_LEN]>>,
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

        lua.set_app_data::<Arc<PreparedRevision>>(prepared.clone());

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

        Self::set_event_declaration(&lua)?;
        Self::set_module_loaders(&lua)?;

        lua.register_extensions(&[
            &JsonExtension,
            &UuidExtension,
            &crate::extensions::http::HttpExtension { egress },
            &KvExtension {
                kv_client: kv_client.clone(),
                project_id: prepared.script.project_id.clone(),
            },
            &crate::extensions::secrets::SecretsExtension {
                kv_client,
                project_id: prepared.script.project_id.clone(),
                key: secrets_key,
            },
            &crate::extensions::log::LogExtension { publisher: logs },
            &JwtExtension,
            &CryptoExtension,
            &crate::extensions::objects::ObjectExtension,
        ])?;

        lua.globals().set(
            "script",
            lua.to_value(&ScriptInfo {
                identifier: prepared.script.public_identifier.clone(),
                project_id: prepared.script.project_id.clone(),
            })?,
        )?;

        let entry_point = prepared.entry_module().transpose()?;

        // We need to set a new timer temporarily when registering.
        // This should be one second since nothing should be happening in this time.
        let original_timer = lua.timer.read().unwrap().clone();
        *lua.timer.write().unwrap() = Timer {
            start_time: Some(Instant::now()),
            time_limit: Some(1),
        };

        // Declarations exist only while the entry point's top level runs;
        // afterwards the same calls error, which is what keeps the code
        // extractable as a manifest.
        lua.set_app_data(Declarations::default());
        lua.set_app_data(DeclarationPhase(true));

        // Run entry point and register handlers.
        if let Some(entry_point) = entry_point {
            let _: () = lua.load(&entry_point).eval_async().await?;
        }

        lua.set_app_data(DeclarationPhase(false));

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

/// Lua module, named by its bundle path so tracebacks point at something a
/// user can find.
pub struct LuaModule {
    /// Name or path of the module.
    pub name: String,
    /// What the vm loads.
    pub code: ModuleCode,
}

/// Code of a [`LuaModule`], compiled ahead of time when possible.
pub enum ModuleCode {
    /// Raw source, for files [`PreparedRevision::prepare`] could not compile.
    Source(String),
    /// Bytecode shared with the revision cache, loaded without recompiling.
    Bytecode(Arc<Vec<u8>>),
}

impl AsChunk for &LuaModule {
    fn source<'a>(&self) -> std::io::Result<Cow<'a, [u8]>>
    where
        Self: 'a,
    {
        Ok(match &self.code {
            ModuleCode::Source(source) => Cow::Borrowed(source.as_bytes()),
            ModuleCode::Bytecode(code) => Cow::Borrowed(code.as_slice()),
        })
    }

    fn mode(&self) -> Option<ChunkMode> {
        Some(match &self.code {
            ModuleCode::Source(_) => ChunkMode::Text,
            ModuleCode::Bytecode(_) => ChunkMode::Binary,
        })
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

    /// Prepares a revision out of `files`, as the revision cache would.
    fn prepared_with(files: Vec<File>) -> Arc<PreparedRevision> {
        let revision = Revision {
            bundle: Some(Bundle {
                entry_point: "main.lua".to_owned(),
                files,
            }),
            ..Default::default()
        };

        Arc::new(PreparedRevision::prepare(Script::default(), revision).expect("prepares"))
    }

    #[test]
    fn assets_resolve_by_exact_path_and_directory_index() {
        let asset = |path: &str| File {
            file_path: path.to_owned(),
            content: b"<h1>hi</h1>".to_vec(),
            kind: FileKind::Asset as i32,
            ..Default::default()
        };
        let prepared = prepared_with(vec![
            File {
                file_path: "main.lua".to_owned(),
                content: b"return 1".to_vec(),
                ..Default::default()
            },
            asset("index.html"),
            asset("docs/index.html"),
            asset("docs/guide.html"),
        ]);

        // Exact paths resolve; directory paths fall back to their index.
        assert!(prepared.asset("docs/guide.html").is_some());
        assert_eq!(
            prepared.asset("").map(|f| f.file_path.as_str()),
            Some("index.html")
        );
        assert_eq!(
            prepared.asset("docs/").map(|f| f.file_path.as_str()),
            Some("docs/index.html")
        );

        // A module is never handed out as an asset, and a directory path
        // does not match its index without the separator.
        assert!(prepared.asset("main.lua").is_none());
        assert!(prepared.asset("docs").is_none());
    }

    /// Builds a lua state carrying `files` as its prepared revision, with the
    /// module loaders installed and no service clients.
    fn runtime_with(files: Vec<File>) -> Lua {
        let lua = Lua::new();
        lua.set_app_data(prepared_with(files));
        ActiasRuntime::set_module_loaders(&lua).expect("module loaders install");
        lua
    }

    fn lua_file(path: &str, source: &str) -> File {
        File {
            file_path: path.to_owned(),
            content: source.as_bytes().to_vec(),
            ..Default::default()
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

    #[test]
    fn prepare_compiles_lua_files_ahead_of_time() {
        // If this regresses to source, every request pays compilation again
        // and the revision cache stops being a bytecode cache.
        let prepared = prepared_with(vec![lua_file("lib/counter.lua", "return 1")]);

        let module = prepared
            .module_by_key("lib.counter")
            .expect("module resolves")
            .expect("module loads");

        assert!(
            matches!(module.code, ModuleCode::Bytecode(_)),
            "lua file was not compiled at prepare time"
        );
    }

    #[tokio::test]
    async fn a_file_that_does_not_compile_fails_at_require_not_prepare() {
        // A broken file a script never loads must not take down the whole
        // revision; the error surfaces only when the file is actually loaded.
        let lua = runtime_with(vec![
            lua_file("ok.lua", "return 1"),
            lua_file("broken.lua", "this is ((( not lua"),
        ]);

        let ok: i64 = lua
            .load("return require(\"ok\")")
            .eval_async()
            .await
            .unwrap();
        assert_eq!(ok, 1);

        let broken: mlua::Result<mlua::Value> =
            lua.load("return require(\"broken\")").eval_async().await;
        assert!(broken.is_err(), "expected a syntax error, got {broken:?}");
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

    /// A full runtime over `source` as main.lua, with unconnectable clients.
    async fn runtime_running(source: &str) -> mlua::Result<ActiasRuntime> {
        let channel = tonic::transport::Channel::from_static("http://127.0.0.1:1").connect_lazy();

        ActiasRuntime::new(
            prepared_with(vec![lua_file("main.lua", source)]),
            crate::proto::kv_service::kv_service_client::KvServiceClient::new(channel),
            crate::egress::EgressClient::new(crate::egress::EgressPolicy::new([], false))
                .expect("client builds"),
            None,
            None,
            None,
        )
        .await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn declarations_at_the_top_level_are_recorded() {
        let lua = runtime_running(
            r#"
            local visits = kv "visits"
            local sessions = kv "sessions"
            on "fetch" (function(request) return { body = "ok" } end)
            "#,
        )
        .await
        .expect("entry point runs");

        let declarations = lua.declarations();
        assert_eq!(declarations.kv, vec!["visits", "sessions"]);
        assert_eq!(declarations.events, vec!["fetch"]);

        // The registered handler is the one the server will call.
        lua.listener(ActiasRuntime::FETCH_EVENT)
            .expect("the fetch handler is registered");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn declarations_inside_a_handler_error_at_request_time() {
        // Minting a capability per request would make the code inextractable
        // as a manifest, so it must fail loudly, not work quietly.
        let lua = runtime_running(
            r#"
            on "fetch" (function(request)
                local sneaky = kv "sneaky"
                return { body = "unreachable" }
            end)
            "#,
        )
        .await
        .expect("entry point runs");

        let listener = lua.listener(ActiasRuntime::FETCH_EVENT).expect("handler");
        let result: mlua::Result<mlua::Value> = listener.call_async(mlua::Value::Nil).await;

        let error = result.expect_err("a handler-time declaration must fail");
        assert!(
            error.to_string().contains("top level"),
            "wrong error: {error}"
        );
    }

    /// A full runtime whose revision carries a published contract.
    async fn runtime_with_contract(
        source: &str,
        kv: &[&str],
        secrets: &[&str],
    ) -> mlua::Result<ActiasRuntime> {
        let channel = tonic::transport::Channel::from_static("http://127.0.0.1:1").connect_lazy();

        let revision = Revision {
            script_config: Some(crate::proto::script_service::ScriptConfig {
                capabilities: Some(crate::proto::script_service::Capabilities {
                    kv: kv.iter().map(|s| s.to_string()).collect(),
                    events: vec!["fetch".to_owned()],
                    secrets: secrets.iter().map(|s| s.to_string()).collect(),
                    objects: vec![],
                }),
                ..Default::default()
            }),
            bundle: Some(Bundle {
                entry_point: "main.lua".to_owned(),
                files: vec![lua_file("main.lua", source)],
            }),
            ..Default::default()
        };

        ActiasRuntime::new(
            Arc::new(PreparedRevision::prepare(Script::default(), revision)?),
            crate::proto::kv_service::kv_service_client::KvServiceClient::new(channel),
            crate::egress::EgressClient::new(crate::egress::EgressPolicy::new([], false))
                .expect("client builds"),
            None,
            None,
            None,
        )
        .await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_declaration_outside_the_contract_fails_the_entry_point() {
        // The contract came from this code at publish; disagreement means the
        // bundle changed without a publish, which must not mint capabilities.
        let result =
            runtime_with_contract(r#"local sneaky = kv "sneaky""#, &["allowed"], &[]).await;

        let Err(error) = result else {
            panic!("an uncontracted namespace must fail")
        };
        assert!(
            error.to_string().contains("capability contract"),
            "wrong error: {error}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn contracted_declarations_run_and_contractless_revisions_are_unenforced() {
        // The happy path: the declared name is in the contract.
        runtime_with_contract(r#"local visits = kv "allowed""#, &["allowed"], &[])
            .await
            .expect("a contracted declaration runs");

        // No contract stored (live sessions, pre-contract revisions): any
        // declaration is honored.
        runtime_running(r#"local visits = kv "anything""#)
            .await
            .expect("a contract-less revision stays unenforced");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_uncontracted_secret_fails_before_any_lookup() {
        // The contract check precedes configuration and network access, so
        // this fails with the contract error even with no key and no backend.
        let result =
            runtime_with_contract(r#"local token = secret "sneaky""#, &[], &["allowed"]).await;

        let Err(error) = result else {
            panic!("an uncontracted secret must fail")
        };
        assert!(
            error.to_string().contains("capability contract"),
            "wrong error: {error}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_unknown_event_declaration_fails_the_entry_point() {
        let result = runtime_running(r#"on "teleport" (function() end)"#).await;

        assert!(result.is_err(), "expected an invalid event error");
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
