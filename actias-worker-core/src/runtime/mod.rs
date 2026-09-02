//! The Luau vm and everything installed into it: the module loader over
//! a bundle, the extension surface a profile admits, the declaration
//! phase that runs the entry point, and the contract checks every
//! capability passes through.
//!
//! One runtime owns one vm. What runs in it (a request, an object call,
//! a connection frame) is decided by the caller; this module only
//! decides what such code may reach.

pub mod extension;

use crate::{
    extensions::{crypto::CryptoExtension, jwt::JwtExtension, kv::KvExtension},
    proto::{
        bundle::{Bundle, File, FileKind},
        kv_service::kv_service_client::KvServiceClient,
        script_service::{Revision, Script},
        secret_service::secret_service_client::SecretServiceClient,
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
    sync::Arc,
};

/// Lua runtime with actias specific methods.
pub struct ActiasRuntime {
    lua: Lua,
    /// What this vm has spent and may still spend, shared with the
    /// interrupt that charges it ([`crate::budget`]).
    meter: Arc<crate::budget::Meter>,
    /// The backstop for one armed scope; work is the real ceiling.
    wall_limit: Option<std::time::Duration>,
    /// Work a single armed scope may spend. Defaults to
    /// [`crate::budget::DEFAULT_WORK_LIMIT`]; a host overrides it from
    /// its own config with [`Self::set_work_limit`].
    work_limit: std::sync::atomic::AtomicU64,
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

/// The name half of a capability entry, which may carry a suffix after
/// '=' (a publish policy, a migrations directory).
fn bare_name(entry: &str) -> String {
    entry
        .split_once('=')
        .map(|(head, _)| head.to_owned())
        .unwrap_or_else(|| entry.to_owned())
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

/// Marker: this vm is a read-only shell session (see
/// [`ActiasRuntime::set_read_only_session`]).
pub struct ReadOnlySession;

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
    /// Names handed to `database "name"`.
    pub databases: Vec<String>,
    /// Names handed to `queue "name"`.
    pub queues: Vec<String>,
    /// Names handed to `workflow "name"`.
    pub workflows: Vec<String>,
    /// Topics from class tables' `publishes` keys, as "Class:topic" or
    /// "Class:topic=policy".
    pub publishes: Vec<String>,
    /// Class lifecycle declarations, "Class:expire=30d" and
    /// "Class:admit".
    pub lifecycle: Vec<String>,
    /// Classes handed to `connection "Class"`.
    pub connections: Vec<String>,
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
    databases: HashSet<String>,
    queues: HashSet<String>,
    /// "Class:topic" entries (policy suffix stripped): what
    /// `state:publish` may emit under this contract.
    publishes: HashSet<String>,
    /// Kept as declared (ordered, duplicates meaningless but harmless);
    /// cron arming reads these.
    events: Vec<String>,
    /// Lifecycle entries as declared, "Class:expire=30d" and
    /// "Class:admit".
    lifecycle: Vec<String>,
    /// Connection classes the code declared; recorded for the console
    /// and check, not yet enforced at declaration (the workflow rule).
    #[allow(dead_code)]
    connections: HashSet<String>,
}

/// Which contract list a declaration checks against.
pub enum ContractKind {
    Kv,
    Secret,
    Object,
    Database,
    Queue,
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
                objects: capabilities
                    .objects
                    .iter()
                    .map(|entry| bare_name(entry))
                    .collect(),
                databases: capabilities
                    .databases
                    .iter()
                    .map(|entry| bare_name(entry))
                    .collect(),
                queues: capabilities.queues.into_iter().collect(),
                publishes: capabilities
                    .publishes
                    .iter()
                    .map(|entry| bare_name(entry))
                    .collect(),
                events: capabilities.events,
                lifecycle: capabilities.lifecycle,
                connections: capabilities.connections.into_iter().collect(),
            });

        Ok(Self {
            script,
            revision_id,
            bundle,
            bytecode,
            contract,
        })
    }

    /// The lifespan the contract declares for `class`, in seconds;
    /// [`None`] means the class never expires. The claim stamps this.
    pub fn expire_secs_for(&self, class: &str) -> Option<u64> {
        let contract = self.contract.as_ref()?;
        let prefix = format!("{class}:expire=");
        let raw = contract
            .lifecycle
            .iter()
            .find_map(|entry| entry.strip_prefix(&prefix))?;
        let ms = actias_declarations::duration::parse_duration_ms(raw).ok()?;
        u64::try_from(ms / 1000).ok().filter(|secs| *secs > 0)
    }

    /// The directory field set the contract declares for `class`, with
    /// the version publish minted for it. [`None`] for a class with no
    /// directory, and for a contract from before fields were declared
    /// (the bare `Class:directory` marker), which derives at version
    /// zero and unvalidated, exactly as it always did.
    pub fn directory_spec(
        &self,
        class: &str,
    ) -> Option<actias_common::directory_spec::DirectorySpec> {
        let contract = self.contract.as_ref()?;
        contract.lifecycle.iter().find_map(|entry| {
            actias_common::directory_spec::DirectorySpec::parse(entry)
                .filter(|(owner, _)| owner == class)
                .map(|(_, spec)| spec)
        })
    }

    /// Whether the contract declares a creation gate for `class`.
    pub fn gates_admission(&self, class: &str) -> bool {
        self.contract.as_ref().is_some_and(|contract| {
            contract
                .lifecycle
                .iter()
                .any(|e| e == &format!("{class}:admit"))
        })
    }

    /// Whether the stored contract admits `class` publishing `topic`;
    /// contract-less revisions (live sessions) stay unenforced.
    pub(crate) fn contract_allows_publish(&self, class: &str, topic: &str) -> bool {
        match &self.contract {
            None => true,
            Some(contract) => contract.publishes.contains(&format!("{class}:{topic}")),
        }
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

    /// The cron events this revision's contract declares; the worker arms
    /// a `__cron` object for each at first touch.
    pub fn cron_events(&self) -> Vec<String> {
        self.contract
            .as_ref()
            .map(|contract| {
                contract
                    .events
                    .iter()
                    .filter(|event| event.starts_with("cron:"))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Migration files for one database, in application order: every
    /// `migrations/<database>/*.sql` in the bundle, sorted by path, which
    /// is why the scaffold numbers them.
    /// The `.sql` files directly under `dir`, sorted by path, which is
    /// why the scaffold numbers them.
    pub fn migrations_in(&self, dir: &str) -> Vec<(String, String)> {
        let prefix = format!("{}/", dir.trim_end_matches('/'));

        let mut files: Vec<(String, String)> = self
            .bundle
            .files
            .iter()
            .filter(|file| file.file_path.starts_with(&prefix) && file.file_path.ends_with(".sql"))
            .map(|file| {
                (
                    file.file_path.clone(),
                    String::from_utf8_lossy(&file.content).into_owned(),
                )
            })
            .collect();
        files.sort();
        files
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

/// Which extension surface a vm gets. Standard is every script vm;
/// Workflow is the enforced-determinism profile, where effects are
/// refused outside steps and ambient nondeterminism journals.
#[derive(Clone)]
pub enum VmProfile {
    Standard,
    /// The enforced-determinism surface; carries the instance's
    /// determinism source (the entry point runs during construction and
    /// every read from the first line on must journal) and its secret
    /// pins, so `secret` declarations resolve the versions the run
    /// started with. [`None`] pins resolve heads: tests without storage.
    Workflow {
        source: std::sync::Arc<dyn crate::extensions::determinism::Determinism>,
        secret_pins: Option<std::sync::Arc<crate::platform::workflow::SecretPins>>,
    },
}

impl ActiasRuntime {
    /// Registry key holding the table of modules loaded so far.
    const MODULE_REGISTRY: &'static str = "module_registry";

    /// Event fired for an inbound http request.
    pub const FETCH_EVENT: &'static str = "fetch";

    /// Events a script may register a listener for.
    /// The shell's one handler: a chunk registers itself under this so
    /// it runs under a call budget rather than the entry's.
    pub const SHELL_EVENT: &'static str = "shell";
    const EVENTS: [&'static str; 2] = [Self::FETCH_EVENT, Self::SHELL_EVENT];

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
    /// The registered listener for `event`, resolved from any code that
    /// holds the [`Lua`], not just the runtime wrapper; the cron class
    /// fires listeners from inside a pinned vm through this.
    pub fn listener_in(lua: &Lua, event: &str) -> mlua::Result<mlua::Function> {
        lua.named_registry_value(&Self::listener_key(event))
    }

    pub fn listener(&self, event: &str) -> mlua::Result<mlua::Function> {
        self.lua.named_registry_value(&Self::listener_key(event))
    }

    /// Errors unless the vm is evaluating the entry point's top level.
    ///
    /// Every declaration form calls this first, so `kv "x"` inside a handler
    /// fails with the same message everywhere.
    ///
    /// # Errors
    /// Returns [`mlua::Error::RuntimeError`] naming `form` once the entry
    /// point has run.
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
    ///
    /// # Errors
    /// Returns [`mlua::Error::RuntimeError`] naming the resource when the
    /// revision's contract does not carry it.
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
            ContractKind::Database => (&contract.databases, "Database"),
            ContractKind::Queue => (&contract.queues, "Queue"),
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

    /// Notes a `connection "Class"` declaration for
    /// [`Self::declarations`].
    pub fn record_connection_declaration(lua: &Lua, class: &str) {
        if let Some(mut declarations) = lua.app_data_mut::<Declarations>() {
            declarations.connections.push(class.to_owned());
        }
    }

    /// Notes one class's `publishes` entry ("Class:topic" or
    /// "Class:topic=policy") for [`Self::declarations`].
    pub fn record_publishes_declaration(lua: &Lua, entry: String) {
        if let Some(mut declarations) = lua.app_data_mut::<Declarations>() {
            declarations.publishes.push(entry);
        }
    }

    /// Notes a `database "name"` declaration for [`Self::declarations`].
    pub fn record_database_declaration(lua: &Lua, name: &str) {
        if let Some(mut declarations) = lua.app_data_mut::<Declarations>() {
            declarations.databases.push(name.to_owned());
        }
    }

    /// Notes a class's declared migrations directory, spelled the way
    /// the contract stores it.
    pub fn record_object_migrations(lua: &Lua, class: &str, dir: &str) {
        if let Some(mut declarations) = lua.app_data_mut::<Declarations>() {
            declarations
                .objects
                .retain(|entry| bare_name(entry) != class);
            declarations.objects.push(format!("{class}={dir}"));
        }
    }

    /// Notes a `queue "name"` declaration for [`Self::declarations`].
    pub fn record_queue_declaration(lua: &Lua, name: &str) {
        if let Some(mut declarations) = lua.app_data_mut::<Declarations>() {
            declarations.queues.push(name.to_owned());
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

                // Fixed names, plus `cron:<expr>` schedules, whose
                // expression must parse or the publish-time pass (and this
                // vm) refuses the declaration outright.
                if let Some(_expr) = event.strip_prefix("cron:") {
                    crate::extensions::objects::cron_delay_ms(&event)
                        .map_err(mlua::Error::RuntimeError)?;
                } else if let Some(name) = event.strip_prefix("queue:") {
                    if name.trim().is_empty() {
                        return Err(mlua::Error::RuntimeError(
                            "A queue event names its queue: on \"queue:<name>\".".to_owned(),
                        ));
                    }
                } else if !Self::EVENTS.contains(&event.as_str()) {
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

    /// The listener key prefix workflow definitions register under; the
    /// platform class fires `workflow:<definition>` when a run executes.
    pub const WORKFLOW_EVENT_PREFIX: &'static str = "workflow:";

    /// `workflow "name" (fn)` declares a durable workflow definition: the
    /// function is the run body, registered like an event listener, and
    /// the name joins the contract so publish-time passes and the console
    /// can see it.
    fn set_workflow_declaration(lua: &Lua) -> mlua::Result<()> {
        lua.globals().set(
            "workflow",
            lua.create_function(|lua, name: String| {
                Self::assert_declaration_phase(lua, "workflow")?;
                if name.trim().is_empty() || name.contains('/') {
                    return Err(mlua::Error::RuntimeError(
                        "A workflow name is a non-empty string without '/'.".to_owned(),
                    ));
                }

                if let Some(mut declarations) = lua.app_data_mut::<Declarations>() {
                    declarations.workflows.push(name.clone());
                }

                lua.create_function(move |lua, callback: mlua::Function| {
                    lua.set_named_registry_value(
                        &Self::listener_key(&format!("{}{name}", Self::WORKFLOW_EVENT_PREFIX)),
                        callback,
                    )?;
                    // The declaration hands back the same handle
                    // `workflows "name"` mints for cross-script callers.
                    crate::extensions::objects::workflow_definition_handle(lua, name.clone())
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

    /// Creates a runtime and runs the bundle's entry point, so every
    /// top-level declaration is registered before the first call.
    ///
    /// # Arguments
    /// - `prepared` - Compiled revision, shared with the cache; the vm loads its bytecode without recompiling.
    /// - `kv_client` - Key value service client, allows the script to access/store persistent data.
    /// - `egress` - Guarded http client for the script's outbound requests.
    /// - `logs` - Where the script's log output goes; [`None`] keeps it in worker tracing only.
    /// - `secret_client` - Secret service resolving `secret` declarations; [`None`] makes them error.
    /// - `time_limit` - Total Time limit in seconds, this is based on seconds and will start when [`start_timer`] is called
    pub async fn new(
        prepared: Arc<PreparedRevision>,
        kv_client: KvServiceClient<crate::Grpc>,
        egress: crate::egress::EgressClient,
        logs: Option<crate::extensions::log::LogPublisher>,
        secret_client: Option<SecretServiceClient<crate::Grpc>>,
        time_limit: Option<u64>,
    ) -> mlua::Result<Self> {
        Self::with_profile(
            prepared,
            kv_client,
            egress,
            logs,
            secret_client,
            time_limit,
            VmProfile::Standard,
        )
        .await
    }

    /// Like [`Self::new`], but choosing the vm's extension profile. The
    /// workflow profile is the enforced-determinism surface: effects are
    /// refused by name, time and uuids journal through the vm's
    /// [`crate::extensions::determinism::DeterminismSource`], and
    /// `math.random` runs off the instance seed applied by
    /// [`Self::apply_determinism`].
    pub async fn with_profile(
        prepared: Arc<PreparedRevision>,
        kv_client: KvServiceClient<crate::Grpc>,
        egress: crate::egress::EgressClient,
        logs: Option<crate::extensions::log::LogPublisher>,
        secret_client: Option<SecretServiceClient<crate::Grpc>>,
        time_limit: Option<u64>,
        profile: VmProfile,
    ) -> mlua::Result<Self> {
        trace!("Initializing lua runtime");

        let lua = Self {
            // catch_rust_panics stays at its default (true), which keeps
            // Luau's native pcall/xpcall: the false setting swaps in
            // mlua's plain-C wrappers, and a plain C frame cannot host a
            // yield, so every user pcall around a cross-object or
            // platform call died with "attempt to yield across
            // metamethod/C-call boundary". Native pcall is yieldable.
            // The trade: a script pcall could observe a Rust panic as a
            // catchable error instead of it resuming immediately; panics
            // still resume once control returns to Rust, and panics in
            // request paths are bugs by house rule anyway.
            lua: Lua::new_with(mlua::StdLib::ALL_SAFE, mlua::LuaOptions::new())?,
            meter: Arc::new(crate::budget::Meter::default()),
            wall_limit: time_limit.map(std::time::Duration::from_secs),
            work_limit: std::sync::atomic::AtomicU64::new(crate::budget::DEFAULT_WORK_LIMIT),
        };

        lua.set_app_data::<Arc<PreparedRevision>>(prepared.clone());

        lua.sandbox(true)?;

        // 128 MB memory limit.
        lua.set_memory_limit(128 * 1000000)?;

        // Luau fires this while guest code executes, so counting fires
        // IS the work meter: measured, a loop iteration costs one tick
        // and a two-hundred millisecond await costs eight, so waiting on
        // io is free and computing is not. mlua offers no instruction
        // hook under Luau (`set_hook` is cfg'd out), and this needs
        // none. The clock survives inside the meter as a sampled
        // backstop (crate::budget).
        let meter = lua.meter.clone();
        lua.set_interrupt(move |_| match meter.tick() {
            Ok(()) => Ok(mlua::VmState::Continue),
            Err(exhausted) => Err(mlua::Error::RuntimeError(exhausted.to_string())),
        });

        Self::set_event_declaration(&lua)?;
        Self::set_workflow_declaration(&lua)?;
        Self::set_module_loaders(&lua)?;

        match profile {
            VmProfile::Standard => {
                lua.register_extensions(&[
                    &JsonExtension,
                    &UuidExtension,
                    &crate::extensions::http::HttpExtension { egress },
                    &KvExtension {
                        kv_client: kv_client.clone(),
                        project_id: prepared.script.project_id.clone(),
                    },
                    &crate::extensions::secrets::SecretsExtension {
                        secret_client,
                        project_id: prepared.script.project_id.clone(),
                        script_id: prepared.script.id.clone(),
                        pins: None,
                    },
                    &crate::extensions::log::LogExtension { publisher: logs },
                    &JwtExtension,
                    &CryptoExtension,
                    &crate::extensions::objects::ObjectExtension,
                    &crate::extensions::sockets::ConnectionExtension,
                ])?;
            }
            // Workflow code keeps json and log; every effect surface is
            // refused by name at the boundary, and uuid journals. The
            // step verb (W3) builds effect contexts with the standard
            // profile instead of re-admitting these here.
            VmProfile::Workflow {
                source,
                secret_pins,
            } => {
                use crate::extensions::determinism::{ForbiddenExtension, JournaledUuidExtension};
                lua.set_app_data(crate::extensions::determinism::DeterminismSource(source));
                // Declaration surfaces (kv, queue, database, object)
                // stay: they run at script top level during replay and
                // only return handles. Guarding the handles' effectful
                // methods is the step verb's dispatch-level business.
                lua.register_extensions(&[
                    &JsonExtension,
                    &JournaledUuidExtension,
                    &KvExtension {
                        kv_client: kv_client.clone(),
                        project_id: prepared.script.project_id.clone(),
                    },
                    &crate::extensions::secrets::SecretsExtension {
                        secret_client,
                        project_id: prepared.script.project_id.clone(),
                        script_id: prepared.script.id.clone(),
                        pins: secret_pins,
                    },
                    &crate::extensions::log::LogExtension { publisher: logs },
                    &crate::extensions::objects::ObjectExtension,
                    &crate::extensions::sockets::ConnectionExtension,
                    &ForbiddenExtension { name: "http" },
                    &ForbiddenExtension { name: "jwt" },
                    &ForbiddenExtension { name: "crypto" },
                ])?;
                crate::extensions::determinism::shim_stdlib(&lua.lua)?;
            }
        }

        lua.globals().set(
            "script",
            lua.to_value(&ScriptInfo {
                identifier: prepared.script.public_identifier.clone(),
                project_id: prepared.script.project_id.clone(),
            })?,
        )?;

        let entry_point = prepared.entry_module().transpose()?;

        // Top-level evaluation gets its own scope: declarations are
        // cheap by construction, so anything spending a real budget up
        // here is a runaway rather than a program.
        lua.meter.arm(crate::budget::Budget::new(
            crate::budget::DECLARATION_WORK_LIMIT,
            std::time::Duration::from_secs(1),
        ));

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

        // The vm idles unmetered until a call arms it.
        lua.meter.disarm();

        Ok(lua)
    }

    /// Arms the meter for one bounded piece of work; pair with
    /// [`Self::end_call_budget`]. Pinned vms live indefinitely, so their
    /// budget is per call, never per lifetime.
    ///
    /// `seconds` is the wall backstop for this scope; the work ceiling
    /// comes from [`Self::set_work_limit`] and is what actually stops a
    /// runaway.
    /// Lets declaration forms (`kv "x"`, `database "x"`, `objects "X"`)
    /// run outside the entry point's top level. For a shell session,
    /// whose principal is the operator rather than a published script:
    /// its statements bind resources as they go, and the contract they
    /// are checked against is the one the session was granted.
    pub fn allow_declarations(&self, allowed: bool) {
        self.lua.set_app_data(DeclarationPhase(allowed));
    }

    /// Marks this vm as a read-only shell session, or clears the mark.
    /// The writing verbs (kv set and delete, a database's exec, any
    /// user object's method) refuse while it is set; the reads keep
    /// working, which is what lets an ad hoc join run without write
    /// mode.
    pub fn set_read_only_session(&self, read_only: bool) {
        if read_only {
            self.lua.set_app_data(ReadOnlySession);
        } else {
            self.lua.remove_app_data::<ReadOnlySession>();
        }
    }

    /// Errors when the vm is a read-only shell session; the writing
    /// verbs call this first.
    ///
    /// # Errors
    /// Returns [`mlua::Error::RuntimeError`] naming `what` while the vm is
    /// inside a read-only window.
    pub fn assert_writes_allowed(lua: &Lua, what: &str) -> mlua::Result<()> {
        if lua.app_data_ref::<ReadOnlySession>().is_some() {
            return Err(mlua::Error::RuntimeError(format!(
                "{what} is a write, and this shell session is read-only; \\write allows it"
            )));
        }
        Ok(())
    }

    pub fn begin_call_budget(&self, seconds: u64) {
        self.meter.arm(crate::budget::Budget::new(
            self.work_limit.load(std::sync::atomic::Ordering::Relaxed),
            std::time::Duration::from_secs(seconds),
        ));
    }

    /// Arms a budget in milliseconds, for guest code the platform runs
    /// on its own behalf and needs to stay short: the directory row's
    /// derivation runs on the object's pinned task, so its budget is
    /// also the bound on how long it may stall the mailbox.
    pub fn begin_short_budget(&self, millis: u64) {
        self.meter.arm(crate::budget::Budget::new(
            self.work_limit.load(std::sync::atomic::Ordering::Relaxed),
            std::time::Duration::from_millis(millis),
        ));
    }

    /// Disarms the per-call budget; the vm idles unmetered until the next
    /// call arms it again.
    pub fn end_call_budget(&self) {
        self.meter.disarm();
    }

    /// Arms the scope a request runs in, using the vm's configured
    /// ceilings.
    pub fn start_timer(&self) {
        self.meter.arm(crate::budget::Budget {
            work: Some(self.work_limit.load(std::sync::atomic::Ordering::Relaxed)),
            wall: self.wall_limit,
        });
    }

    /// Sets how much work one scope may spend, from the host's config.
    pub fn set_work_limit(&self, units: u64) {
        self.work_limit
            .store(units, std::sync::atomic::Ordering::Relaxed);
    }

    /// What this vm has spent, for metering.
    pub fn consumed(&self) -> crate::budget::Consumed {
        self.meter.consumed()
    }

    /// Registers extensions into the runtime, each under its own name.
    ///
    /// # Errors
    /// Returns [`mlua::Error`] when an extension's value cannot be built or
    /// installed.
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

    /// Sets one module's value, making it visible to `require`.
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
    /// A determinism source with scripted answers, standing in for the
    /// journal cursor W3 installs.
    struct Scripted {
        times: std::sync::Mutex<std::collections::VecDeque<i64>>,
        uuids: std::sync::Mutex<std::collections::VecDeque<String>>,
        rng: std::sync::Mutex<u64>,
    }

    impl crate::extensions::determinism::Determinism for Scripted {
        fn time(&self) -> i64 {
            self.times
                .lock()
                .expect("no poison")
                .pop_front()
                .unwrap_or(0)
        }
        fn uuid(&self) -> String {
            self.uuids
                .lock()
                .expect("no poison")
                .pop_front()
                .unwrap_or_default()
        }
        fn random(&self) -> f64 {
            // xorshift64*, stepped per draw: engine-independent and
            // fully determined by the seed, like the real source.
            let mut state = self.rng.lock().expect("no poison");
            *state ^= *state >> 12;
            *state ^= *state << 25;
            *state ^= *state >> 27;
            let bits = state.wrapping_mul(0x2545F4914F6CDD1D);
            (bits >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    fn scripted(times: &[i64], uuids: &[&str], seed: i64) -> Arc<Scripted> {
        Arc::new(Scripted {
            times: std::sync::Mutex::new(times.iter().copied().collect()),
            uuids: std::sync::Mutex::new(uuids.iter().map(|s| s.to_string()).collect()),
            rng: std::sync::Mutex::new(seed as u64 | 1),
        })
    }

    async fn workflow_running(
        source: &str,
        determinism: Arc<Scripted>,
    ) -> mlua::Result<ActiasRuntime> {
        let channel = tonic::transport::Channel::from_static("http://127.0.0.1:1").connect_lazy();
        ActiasRuntime::with_profile(
            prepared_with(vec![lua_file("main.lua", source)]),
            crate::proto::kv_service::kv_service_client::KvServiceClient::new(crate::plain_grpc(
                channel,
            )),
            crate::egress::EgressClient::new(crate::egress::EgressPolicy::new([], false))
                .expect("client builds"),
            None,
            None,
            None,
            VmProfile::Workflow {
                source: determinism,
                secret_pins: None,
            },
        )
        .await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn workflow_code_cannot_reach_http_outside_a_step() {
        let lua = workflow_running(
            r#"
            on "fetch" (function()
                return http.make_request({ uri = "http://example.com" })
            end)
            "#,
            scripted(&[], &[], 1),
        )
        .await
        .expect("loads");

        let listener = lua.listener("fetch").expect("registered");
        let outcome = listener.call_async::<mlua::Value>(()).await;
        let text = format!("{:#}", outcome.expect_err("must refuse"));
        assert!(
            text.contains(crate::extensions::determinism::FORBIDDEN),
            "wrong refusal: {text}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn shimmed_time_and_uuid_read_from_the_source_in_order() {
        let source = r#"
            on "fetch" (function()
                return { t1 = os.time(), t2 = os.time(), id = uuid.v4() }
            end)
        "#;
        let run = |times: Vec<i64>, uuids: Vec<&'static str>| async move {
            let lua = workflow_running(source, scripted(&times, &uuids, 1))
                .await
                .expect("loads");
            let listener = lua.listener("fetch").expect("registered");
            let value: mlua::Value = listener.call_async(()).await.expect("answers");
            serde_json::to_value(&value).expect("serializes")
        };

        let first = run(vec![100, 250], vec!["id-a"]).await;
        assert_eq!(first["t1"], 100, "{first}");
        assert_eq!(first["t2"], 250, "time journals per read");
        assert_eq!(first["id"], "id-a");

        // Replay: the same scripted answers reproduce the same values,
        // which is the whole promise.
        let replay = run(vec![100, 250], vec!["id-a"]).await;
        assert_eq!(first, replay);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn random_is_seed_stable_and_reseeding_is_refused() {
        let source = r#"
            on "fetch" (function(mode)
                if mode == "reseed" then
                    math.randomseed(42)
                    return {}
                end
                return { math.random(1000000), math.random(1000000) }
            end)
        "#;
        let draw = |seed: i64| async move {
            let lua = workflow_running(source, scripted(&[], &[], seed))
                .await
                .expect("loads");
            let listener = lua.listener("fetch").expect("registered");
            let value: mlua::Value = listener.call_async("draw").await.expect("answers");
            serde_json::to_value(&value).expect("serializes")
        };

        let first = draw(7).await;
        let same_seed = draw(7).await;
        assert_eq!(first, same_seed, "one seed, one sequence");

        let lua = workflow_running(source, scripted(&[], &[], 7))
            .await
            .expect("loads");
        let listener = lua.listener("fetch").expect("registered");
        let refused = listener.call_async::<mlua::Value>("reseed").await;
        let text = format!("{:#}", refused.expect_err("reseeding must refuse"));
        assert!(
            text.contains(crate::extensions::determinism::FORBIDDEN),
            "wrong refusal: {text}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn workflow_handles_route_start_and_run_methods() {
        let lua = runtime_running(
            r#"
            local orders = workflow "fulfill" (function(wf, input)
                return { ok = true }
            end)
            on "fetch" (function()
                local run = orders:start({ n = 7 }, { id = "order-9" })
                local st = run:status()
                return { started = run.started, st = st }
            end)
            "#,
        )
        .await
        .expect("loads");

        // A recording router standing in for the object machinery.
        let seen: Arc<std::sync::Mutex<Vec<(String, String, String)>>> = Arc::default();
        let recorder = seen.clone();
        let router: crate::extensions::objects::ObjectRouter = Arc::new(move |target| {
            let recorder = recorder.clone();
            Box::pin(async move {
                recorder.lock().expect("no poison").push((
                    target.class.clone(),
                    target.name.clone(),
                    target.method.clone(),
                ));
                Ok(serde_json::json!({ "status": "parked", "reason": "test" }))
            })
        });
        lua.set_app_data::<crate::extensions::objects::ObjectRouter>(router);

        let listener = lua
            .listener(ActiasRuntime::FETCH_EVENT)
            .expect("registered");
        let value: mlua::Value = listener.call_async(()).await.expect("answers");
        let answer: serde_json::Value = lua.from_value(value).expect("serializes");
        assert_eq!(answer["started"]["status"], "parked", "{answer}");
        assert_eq!(answer["st"]["status"], "parked");

        let calls = seen.lock().expect("no poison").clone();
        assert_eq!(
            calls,
            vec![
                (
                    "__workflow".to_owned(),
                    "fulfill/order-9".to_owned(),
                    "start".to_owned()
                ),
                (
                    "__workflow".to_owned(),
                    "fulfill/order-9".to_owned(),
                    "status".to_owned()
                ),
            ],
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn starting_without_an_id_is_refused() {
        let lua = runtime_running(
            r#"
            local orders = workflows "fulfill"
            on "fetch" (function()
                return orders:start({ n = 1 })
            end)
            "#,
        )
        .await
        .expect("loads");
        let router: crate::extensions::objects::ObjectRouter =
            Arc::new(|_target| Box::pin(async move { Ok(serde_json::Value::Null) }));
        lua.set_app_data::<crate::extensions::objects::ObjectRouter>(router);

        let listener = lua
            .listener(ActiasRuntime::FETCH_EVENT)
            .expect("registered");
        let refused = listener.call_async::<mlua::Value>(()).await;
        let text = format!("{:#}", refused.expect_err("must refuse"));
        assert!(text.contains("the run's identity"), "wrong refusal: {text}");
    }

    async fn runtime_running(source: &str) -> mlua::Result<ActiasRuntime> {
        let channel = tonic::transport::Channel::from_static("http://127.0.0.1:1").connect_lazy();

        ActiasRuntime::new(
            prepared_with(vec![lua_file("main.lua", source)]),
            crate::proto::kv_service::kv_service_client::KvServiceClient::new(crate::plain_grpc(
                channel,
            )),
            crate::egress::EgressClient::new(crate::egress::EgressPolicy::new([], false))
                .expect("client builds"),
            None,
            None,
            None,
        )
        .await
    }

    /// The knob a host turns: the same loop must survive the default
    /// ceiling and be cut off by a low one, and the refusal must quote
    /// the configured number rather than the constant.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_work_ceiling_is_whatever_the_host_set() {
        let loop_of = |turns: u32| {
            format!(
                r#"
                on "fetch" (function()
                    local n = 0
                    for i = 1, {turns} do n = n + i end
                    return {{ body = tostring(n) }}
                end)
                "#
            )
        };

        let generous = runtime_running(&loop_of(20_000)).await.expect("runs");
        generous.start_timer();
        generous
            .listener(ActiasRuntime::FETCH_EVENT)
            .expect("registered")
            .call_async::<mlua::Value>(())
            .await
            .expect("the default ceiling is not reached by 20k turns");

        let stingy = runtime_running(&loop_of(20_000)).await.expect("runs");
        stingy.set_work_limit(500);
        stingy.start_timer();
        let refused = stingy
            .listener(ActiasRuntime::FETCH_EVENT)
            .expect("registered")
            .call_async::<mlua::Value>(())
            .await;
        let text = format!(
            "{:#}",
            refused.expect_err("500 units cannot cover 20k turns")
        );
        assert!(
            text.contains("too much work") && text.contains("500"),
            "the refusal should name the configured limit: {text}"
        );
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
    async fn a_connection_declaration_registers_and_bad_bodies_refuse() {
        let lua = runtime_running(
            r#"
            local Session = connection "Session" {
                open = function(conn) end,
                frame = function(conn, data) end,
            }
            on "fetch" (function(request) return { body = "ok" } end)
            "#,
        )
        .await
        .expect("entry point runs");

        assert_eq!(lua.declarations().connections, vec!["Session"]);
        let registry = crate::extensions::sockets::ConnectionRegistry::of(&lua);
        let spec = registry.spec("Session").expect("the spec is registered");
        assert_eq!(spec.handlers, vec!["open", "frame"]);
        registry
            .class_table("Session")
            .expect("the body table is reachable");

        let Err(refused) =
            runtime_running(r#"connection "Session" { bid = function() end }"#).await
        else {
            panic!("a method-bearing body must refuse")
        };
        assert!(
            refused.to_string().contains("not a connection handler"),
            "{refused}"
        );
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
                    databases: vec![],
                    queues: vec![],
                    workflows: vec![],
                    workflow_steps: vec![],
                    publishes: vec![],
                    lifecycle: vec![],
                    connections: vec![],
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
            crate::proto::kv_service::kv_service_client::KvServiceClient::new(crate::plain_grpc(
                channel,
            )),
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
