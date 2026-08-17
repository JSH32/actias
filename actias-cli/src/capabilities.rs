//! Declaration-mode extraction: boots the entry point in a stub vm and
//! records what it declares, which becomes the revision's capability
//! contract (docs/SURFACE.md: the code is the manifest).
//!
//! Handlers are registered but never invoked, so no capability is exercised;
//! platform globals are inert stubs that absorb any call or index, and only
//! the declaration forms record.

// Arc/Mutex rather than Rc/RefCell: workspace feature unification can
// switch mlua's send feature on, and these closures must compile either way.
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use base64::Engine;
use mlua::Lua;

use crate::script::ScriptConfig;

/// What the entry point declared.
#[derive(Debug, Clone, Default)]
pub struct Capabilities {
    /// Namespaces declared with `kv "name"`.
    pub kv: Vec<String>,
    /// Events declared with `on "event"`.
    pub events: Vec<String>,
}

/// Ambient globals a script may touch at its top level; each becomes an
/// inert stub. Declaration forms are separate because they record.
const AMBIENT_GLOBALS: [&str; 8] = [
    "json", "uuid", "http", "crypto", "jwt", "log", "script", "getfile",
];

/// Canonical key for a module, mirroring the worker's resolution so a
/// bundle that publishes cleanly also runs: `dir/mod.lua`, `dir/mod` and
/// `dir.mod` all key to `dir.mod`.
fn module_key(name: &str) -> String {
    let name = name.trim_start_matches("./");
    let name = name.strip_suffix(".lua").unwrap_or(name);
    name.replace('/', ".")
}

/// Runs the project's entry point in declaration mode and collects the
/// contract.
///
/// # Errors
/// Returns text describing the failure: a file that does not glob, a syntax
/// error, or a runtime error in top-level code. Failing here is the point;
/// it is the same error the worker would hit on the first request.
pub fn extract(config: &ScriptConfig) -> Result<Capabilities, String> {
    let bundle = config.to_bundle()?;

    let mut files: HashMap<String, String> = HashMap::new();
    for file in &bundle.files {
        let content = base64::engine::general_purpose::STANDARD_NO_PAD
            .decode(&file.content)
            .map_err(|e| format!("{}: {e}", file.file_path))?;

        if let Ok(source) = String::from_utf8(content) {
            files.insert(file.file_path.clone(), source);
        }
    }

    let entry = files
        .iter()
        .find(|(path, _)| path.rsplit('/').next().unwrap_or(path) == config.entry_point.as_str())
        .map(|(path, source)| (path.clone(), source.clone()))
        .ok_or_else(|| format!("Entry point '{}' is not in the bundle", config.entry_point))?;

    let recorded = Arc::new(Mutex::new(Capabilities::default()));

    let lua = Lua::new();
    install_declarations(&lua, &recorded).map_err(|e| e.to_string())?;
    install_stubs(&lua).map_err(|e| e.to_string())?;
    install_loaders(&lua, files).map_err(|e| e.to_string())?;

    lua.load(entry.1.as_str())
        .set_name(entry.0)
        .exec()
        .map_err(|e| format!("Declaration pass failed: {e}"))?;

    let capabilities = recorded.lock().expect("no other holder").clone();
    Ok(capabilities)
}

/// Installs the recording declaration forms: `kv` and `on`.
fn install_declarations(lua: &Lua, recorded: &Arc<Mutex<Capabilities>>) -> mlua::Result<()> {
    let kv_recorded = recorded.clone();
    lua.globals().set(
        "kv",
        lua.create_function(move |lua, namespace: String| {
            kv_recorded
                .lock()
                .expect("no other holder")
                .kv
                .push(namespace);
            stub(lua)
        })?,
    )?;

    let on_recorded = recorded.clone();
    lua.globals().set(
        "on",
        lua.create_function(move |lua, event: String| {
            on_recorded
                .lock()
                .expect("no other holder")
                .events
                .push(event);
            // The registrar accepts the handler and drops it; handlers are
            // never invoked during extraction.
            lua.create_function(|_, _: mlua::Value| Ok(()))
        })?,
    )?;

    Ok(())
}

/// Installs inert stubs for every ambient global, so top-level code that
/// touches the platform surface runs without exercising anything.
fn install_stubs(lua: &Lua) -> mlua::Result<()> {
    for name in AMBIENT_GLOBALS {
        lua.globals().set(name, stub(lua)?)?;
    }

    Ok(())
}

/// A value that absorbs whatever the script does with it: calling it or
/// indexing it yields another stub.
fn stub(lua: &Lua) -> mlua::Result<mlua::Table> {
    let table = lua.create_table()?;
    let meta = lua.create_table()?;

    meta.set(
        "__call",
        lua.create_function(|lua, _: mlua::MultiValue| stub(lua))?,
    )?;
    meta.set(
        "__index",
        lua.create_function(|lua, _: (mlua::Table, mlua::Value)| stub(lua))?,
    )?;

    table.set_metatable(Some(meta))?;
    Ok(table)
}

/// Installs `require`/`dofile` over the bundle, so modules loaded during the
/// entry point's top level run and their declarations record too.
fn install_loaders(lua: &Lua, files: HashMap<String, String>) -> mlua::Result<()> {
    let by_key: HashMap<String, String> = files
        .into_iter()
        .map(|(path, source)| (module_key(&path), source))
        .collect();

    let sources = Arc::new(by_key);
    let loaded = Arc::new(Mutex::new(HashMap::<String, mlua::Value>::new()));

    let require_sources = sources.clone();
    lua.globals().set(
        "require",
        lua.create_function(move |lua, name: String| {
            let key = module_key(&name);

            if let Some(cached) = loaded.lock().expect("no other holder").get(&key) {
                return Ok(cached.clone());
            }

            let Some(source) = require_sources.get(&key) else {
                return Ok(mlua::Value::Nil);
            };

            let value: mlua::Value = lua.load(source.as_str()).set_name(&key).eval()?;
            loaded
                .lock()
                .expect("no other holder")
                .insert(key, value.clone());
            Ok(value)
        })?,
    )?;

    lua.globals().set(
        "dofile",
        lua.create_function(
            move |lua, name: String| match sources.get(&module_key(&name)) {
                Some(source) => lua
                    .load(source.as_str())
                    .set_name(&name)
                    .eval::<mlua::Value>(),
                None => Ok(mlua::Value::Nil),
            },
        )?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A project on disk with the given files, first one the entry point.
    fn project(files: &[(&str, &str)]) -> (tempfile::TempDir, ScriptConfig) {
        let dir = tempfile::tempdir().expect("tempdir");

        for (path, source) in files {
            let full = dir.path().join(path);
            std::fs::create_dir_all(full.parent().expect("parent")).expect("dirs");
            let mut file = std::fs::File::create(full).expect("file");
            file.write_all(source.as_bytes()).expect("write");
        }

        let config: ScriptConfig = serde_json::from_str(&format!(
            r#"{{"id":"00000000-0000-0000-0000-000000000000",
                 "entryPoint":"{}","includes":["**/*.lua"],"ignore":[]}}"#,
            files[0].0
        ))
        .expect("config parses");

        let mut config = config;
        config.project_path = Some(dir.path().to_path_buf());
        (dir, config)
    }

    #[test]
    fn declarations_are_recorded_without_running_handlers() {
        let (_dir, config) = project(&[(
            "main.lua",
            r#"
            local visits = kv "visits"
            local sessions = kv "sessions"

            on "fetch" (function(request)
                error("handlers must never run during extraction")
            end)
            "#,
        )]);

        let capabilities = extract(&config).expect("extraction succeeds");

        assert_eq!(capabilities.kv, vec!["visits", "sessions"]);
        assert_eq!(capabilities.events, vec!["fetch"]);
    }

    #[test]
    fn declarations_in_required_modules_are_recorded() {
        let (_dir, config) = project(&[
            ("main.lua", r#"require "store" on "fetch" (function() end)"#),
            ("store.lua", r#"local cache = kv "cache" return cache"#),
        ]);

        let capabilities = extract(&config).expect("extraction succeeds");

        assert_eq!(capabilities.kv, vec!["cache"]);
    }

    #[test]
    fn ambient_globals_absorb_top_level_use() {
        let (_dir, config) = project(&[(
            "main.lua",
            r#"
            local greeting = json.stringify({ hello = "world" })
            log.info(greeting)
            on "fetch" (function() end)
            "#,
        )]);

        extract(&config).expect("stubs absorb platform calls");
    }

    #[test]
    fn a_syntax_error_fails_the_declaration_pass() {
        let (_dir, config) = project(&[("main.lua", "this is ((( not lua")]);

        let error = extract(&config).expect_err("broken code must fail publish");
        assert!(error.contains("Declaration pass failed"), "{error}");
    }
}
