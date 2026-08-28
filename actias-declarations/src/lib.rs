//! Declaration-mode extraction: boots a script's entry point in a stub vm
//! and records what it declares, which becomes the revision's capability
//! contract. The code is the manifest.
//!
//! Handlers are registered but never invoked, so no capability is exercised;
//! platform globals are inert stubs that absorb any call or index, and only
//! the declaration forms record. The vm carries a wall-clock interrupt and a
//! memory cap, so hostile top-level code cannot hold the extractor hostage.
//!
//! The CLI runs this as a local pre-flight; script-service runs it on every
//! publish and stores what it derives, so a stored contract always equals
//! the code, whoever published it.

// Arc/Mutex rather than Rc/RefCell: workspace feature unification can
// switch mlua's send feature on, and these closures must compile either way.
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use mlua::Lua;
use serde::{Deserialize, Serialize};

mod class_spec;
mod connection_spec;
pub mod duration;
pub use class_spec::{ClassSpec, RESERVED_METHODS, TopicPolicy, callable_method};
pub use connection_spec::{CONNECTION_HANDLERS, ConnectionSpec};

/// Longest a top-level evaluation may run; declarations are cheap, so
/// anything hitting this is a runaway loop.
const EXTRACTION_TIME_LIMIT: Duration = Duration::from_secs(1);

/// Memory cap for the extraction vm.
const EXTRACTION_MEMORY_LIMIT: usize = 64 * 1024 * 1024;

/// What the entry point declared.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct Declarations {
    /// Namespaces declared with `kv "name"`.
    pub kv: Vec<String>,
    /// Events declared with `on "event"`.
    pub events: Vec<String>,
    /// Secrets declared with `secret "name"`.
    pub secrets: Vec<String>,
    /// Object classes declared with `object "Class" { ... }`.
    #[serde(default)]
    pub objects: Vec<String>,
    /// Databases declared with `database "name"`.
    #[serde(default)]
    pub databases: Vec<String>,
    /// Queues declared with `queue "name"`.
    #[serde(default)]
    pub queues: Vec<String>,
    /// Workflow definitions declared with `workflow "name"`.
    #[serde(default)]
    pub workflows: Vec<String>,
    /// Step name literals found in the sources (`wf:step("name", ...)`),
    /// a SUPERSET of what may run: the console's declared-possible
    /// skeleton. Dynamically built names are invisible here and simply
    /// appear as they execute.
    #[serde(default)]
    pub workflow_steps: Vec<String>,
    /// Topics each class publishes, read from the class table's
    /// `publishes` key: "Class:topic" for gate-hooked entries,
    /// "Class:topic=policy" for built-in policies ("self").
    #[serde(default)]
    pub publishes: Vec<String>,
    /// Streams each class consumes, read from the class table's
    /// `receives` key: "Class<-Source:topic", one entry per handler.
    #[serde(default)]
    pub receives: Vec<String>,
    /// Follow targets found as source literals
    /// (`state:follow(Class(...), "topic")`), spelled "Class:topic".
    /// Like workflow_steps, a checker-visible SUPERSET heuristic:
    /// dynamically chosen topics are invisible here.
    #[serde(default)]
    pub follow_sites: Vec<String>,
    /// Class lifecycle declarations: "Class:expire=30d" for a declared
    /// lifespan, "Class:admit" for a creation gate.
    #[serde(default)]
    pub lifecycle: Vec<String>,
    /// Connection classes declared with `connection "Class" { ... }`.
    #[serde(default)]
    pub connections: Vec<String>,
}

/// Ambient globals a script may touch at its top level; each becomes an
/// inert stub. Declaration forms are separate because they record.
const AMBIENT_GLOBALS: [&str; 8] = [
    "json", "uuid", "http", "crypto", "jwt", "log", "script", "getfile",
];

/// Canonical key for a module, mirroring the worker's resolution so a
/// bundle that extracts cleanly also runs: `dir/mod.lua`, `dir/mod` and
/// `dir.mod` all key to `dir.mod`.
fn module_key(name: &str) -> String {
    let name = name.trim_start_matches("./");
    let name = name.strip_suffix(".lua").unwrap_or(name);
    name.replace('/', ".")
}

/// Runs the entry point in declaration mode over in-memory sources and
/// collects the contract.
///
/// `files` maps bundle paths to utf-8 lua source; non-lua files are simply
/// not offered to the loaders.
///
/// # Errors
/// Returns text describing the failure: a missing entry point, a syntax
/// error, or a runtime error in top-level code. Failing here is the point;
/// it is the same error the worker would hit on the first request.
/// Collects `wf:step("name"` string literals from lua source; a scan,
/// not a parse, so it stays a superset and never a promise.
fn scan_step_literals(files: &HashMap<String, String>) -> Vec<String> {
    let mut names = Vec::new();
    for (path, source) in files {
        if !path.ends_with(".lua") {
            continue;
        }
        for window in source.split(":step(") {
            let trimmed = window.trim_start();
            if let Some(rest) = trimmed.strip_prefix('"')
                && let Some(end) = rest.find('"')
            {
                let name = &rest[..end];
                if !name.is_empty() && !names.contains(&name.to_owned()) {
                    names.push(name.to_owned());
                }
            }
        }
    }
    // The first window precedes any match; splitting yields it too, so
    // the loop above only ever sees text AFTER a `:step(`.
    names
}

/// Follow targets written as literals: `state:follow(Class(...), "topic")`
/// yields "Class:topic". Only `state:follow` counts; `sock:follow` edges
/// deliver to a connection inbox, never to a `receives` handler.
fn scan_follow_literals(files: &HashMap<String, String>) -> Vec<String> {
    let mut sites = Vec::new();
    for (path, source) in files {
        if !path.ends_with(".lua") {
            continue;
        }
        for window in source.split("state:follow(") {
            let trimmed = window.trim_start();
            let class: String = trimmed
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if class.is_empty() || !trimmed[class.len()..].trim_start().starts_with('(') {
                continue;
            }
            // The topic is the first string literal after the handle's
            // closing paren: skip past one balanced (...) then read it.
            let Some(after_open) = trimmed[class.len()..].trim_start().strip_prefix('(') else {
                continue;
            };
            let mut depth = 1usize;
            let mut rest = "";
            for (at, c) in after_open.char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            rest = &after_open[at + 1..];
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let rest = rest.trim_start().trim_start_matches(',').trim_start();
            if let Some(quoted) = rest.strip_prefix('"')
                && let Some(end) = quoted.find('"')
            {
                let site = format!("{class}:{}", &quoted[..end]);
                if !quoted[..end].is_empty() && !sites.contains(&site) {
                    sites.push(site);
                }
            }
        }
    }
    sites
}

pub fn extract(files: HashMap<String, String>, entry_point: &str) -> Result<Declarations, String> {
    let entry_source = files
        .iter()
        .find(|(path, _)| path.as_str() == entry_point)
        .map(|(_, source)| source.clone())
        .ok_or_else(|| format!("Entry point '{entry_point}' is not in the bundle"))?;

    let recorded = Arc::new(Mutex::new(Declarations::default()));

    let lua = Lua::new();
    lua.set_memory_limit(EXTRACTION_MEMORY_LIMIT)
        .map_err(|e| e.to_string())?;

    let deadline = Instant::now() + EXTRACTION_TIME_LIMIT;
    lua.set_interrupt(move |_| {
        if Instant::now() > deadline {
            Err(mlua::Error::RuntimeError(
                "Top-level evaluation took too long.".to_owned(),
            ))
        } else {
            Ok(mlua::VmState::Continue)
        }
    });

    install_declarations(&lua, &recorded).map_err(|e| e.to_string())?;
    install_stubs(&lua).map_err(|e| e.to_string())?;
    let step_names = scan_step_literals(&files);
    let follow_sites = scan_follow_literals(&files);
    install_loaders(&lua, files).map_err(|e| e.to_string())?;

    lua.load(entry_source)
        .set_name(entry_point)
        .exec()
        .map_err(|e| format!("Declaration pass failed: {e}"))?;

    let mut declarations = recorded.lock().expect("no other holder").clone();
    declarations.workflow_steps = step_names;
    declarations.follow_sites = follow_sites;
    Ok(declarations)
}

/// Installs the recording declaration forms: `kv`, `secret` and `on`.
fn install_declarations(lua: &Lua, recorded: &Arc<Mutex<Declarations>>) -> mlua::Result<()> {
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

    let secret_recorded = recorded.clone();
    lua.globals().set(
        "secret",
        lua.create_function(move |_, name: String| {
            secret_recorded
                .lock()
                .expect("no other holder")
                .secrets
                .push(name);
            // The real handle is the plaintext; extraction has no values, so
            // scripts get an empty string that behaves like one.
            Ok(String::new())
        })?,
    )?;

    // `object "Class" { methods }` is curried: the name records the class,
    // the table is absorbed and a class-handle stub comes back. `objects`
    // only references a class declared elsewhere, so it records nothing.
    let object_recorded = recorded.clone();
    lua.globals().set(
        "object",
        lua.create_function(move |lua, class: String| {
            object_recorded
                .lock()
                .expect("no other holder")
                .objects
                .push(class.clone());
            let table_recorded = object_recorded.clone();
            lua.create_function(move |lua, body: mlua::Table| {
                // One reader for the class body: the same parse the
                // worker's registry runs, so the stored contract and
                // the enforcing runtime agree by construction.
                let spec = class_spec::ClassSpec::parse(&class, &body)?;
                let mut recorded = table_recorded.lock().expect("no other holder");
                for declared in &spec.declared_publishes {
                    recorded.publishes.push(format!("{class}:{declared}"));
                }
                if let Some(dir) = &spec.migrations {
                    recorded
                        .objects
                        .retain(|entry| entry.split('=').next().unwrap_or(entry) != class);
                    recorded.objects.push(format!("{class}={dir}"));
                }
                for stream in &spec.receives {
                    recorded.receives.push(format!("{class}<-{stream}"));
                }
                if let Some(raw) = &spec.expire_raw {
                    recorded.lifecycle.push(format!("{class}:expire={raw}"));
                }
                if spec.admits {
                    recorded.lifecycle.push(format!("{class}:admit"));
                }
                drop(recorded);
                stub(lua)
            })
        })?,
    )?;

    lua.globals()
        .set("objects", lua.create_function(|lua, _: String| stub(lua))?)?;

    // `connection "Class" { handlers }` is curried like `object`: the
    // name records the class, the body is parsed by the same reader the
    // runtime registry runs, and the handle stub comes back for the
    // upgrade call to hold.
    let connection_recorded = recorded.clone();
    lua.globals().set(
        "connection",
        lua.create_function(move |lua, class: String| {
            connection_recorded
                .lock()
                .expect("no other holder")
                .connections
                .push(class.clone());
            lua.create_function(move |lua, body: mlua::Table| {
                connection_spec::ConnectionSpec::parse(&class, &body)?;
                stub(lua)
            })
        })?,
    )?;

    let database_recorded = recorded.clone();
    lua.globals().set(
        "database",
        lua.create_function(move |lua, name: String| {
            database_recorded
                .lock()
                .expect("no other holder")
                .databases
                .push(name.clone());
            // `database "name" { migrations = "dir" }`: the handle takes
            // an optional body naming where its schema comes from.
            let handle = lua.create_table()?;
            let meta = lua.create_table()?;
            let body_recorded = database_recorded.clone();
            meta.set(
                "__call",
                lua.create_function(move |_, (this, body): (mlua::Table, mlua::Table)| {
                    if let Ok(dir) = body.get::<String>("migrations") {
                        let mut recorded = body_recorded.lock().expect("no other holder");
                        recorded
                            .databases
                            .retain(|entry| entry.split('=').next().unwrap_or(entry) != name);
                        recorded.databases.push(format!("{name}={dir}"));
                    }
                    Ok(this)
                })?,
            )?;
            meta.set(
                "__index",
                lua.create_function(|lua, _: (mlua::Table, mlua::Value)| stub(lua))?,
            )?;
            handle.set_metatable(Some(meta))?;
            Ok(handle)
        })?,
    )?;

    let queue_recorded = recorded.clone();
    lua.globals().set(
        "queue",
        lua.create_function(move |lua, name: String| {
            queue_recorded
                .lock()
                .expect("no other holder")
                .queues
                .push(name);
            stub(lua)
        })?,
    )?;

    let workflow_recorded = recorded.clone();
    lua.globals().set(
        "workflow",
        lua.create_function(move |lua, name: String| {
            // The same shape the runtime enforces; a bad name dies at
            // publish, not on the first start.
            if name.trim().is_empty() || name.contains('/') {
                return Err(mlua::Error::RuntimeError(
                    "A workflow name is a non-empty string without '/'.".to_owned(),
                ));
            }
            workflow_recorded
                .lock()
                .expect("no other holder")
                .workflows
                .push(name);
            // `workflow "name" (fn)`: the registrar takes the body and
            // returns the handle stub callers hold.
            lua.create_function(|lua, _body: mlua::Function| stub(lua))
        })?,
    )?;

    // `workflows "name"` is a lookup, not a declaration: it records
    // nothing and hands back a handle stub.
    lua.globals().set(
        "workflows",
        lua.create_function(move |lua, _name: String| stub(lua))?,
    )?;

    let on_recorded = recorded.clone();
    lua.globals().set(
        "on",
        lua.create_function(move |lua, event: String| {
            // A cron schedule that cannot parse must die here, at publish,
            // not on the first request the revision ever serves.
            if let Some(expr) = event.strip_prefix("cron:") {
                validate_cron(expr).map_err(mlua::Error::RuntimeError)?;
            }
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

/// A cron expression scripts may schedule on: five classic fields or six
/// with seconds; the parser wants six, so five gain a zero.
fn validate_cron(expr: &str) -> Result<(), String> {
    use std::str::FromStr;

    let expr = expr.trim();
    let normalized = if expr.split_whitespace().count() == 5 {
        format!("0 {expr}")
    } else {
        expr.to_owned()
    };

    cron::Schedule::from_str(&normalized)
        .map(|_| ())
        .map_err(|e| format!("'{expr}' is not a cron expression: {e}"))
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

    fn files(entries: &[(&str, &str)]) -> HashMap<String, String> {
        entries
            .iter()
            .map(|(path, source)| (path.to_string(), source.to_string()))
            .collect()
    }

    #[test]
    fn publishes_entries_ride_the_object_declaration() {
        let declarations = extract(
            files(&[(
                "main.lua",
                r#"
                local Chan = object "Chan" {
                    publishes = { "message", "activity", events = "self" },
                    hooks = {
                        follow = function(state, topic, follower)
                            error("gates must never run during extraction")
                        end,
                    },
                }
                local Plain = object "Plain" {
                    poke = function(state) end,
                }
                on "fetch" (function() end)
                "#,
            )]),
            "main.lua",
        )
        .expect("extracts");

        let mut publishes = declarations.publishes.clone();
        publishes.sort();
        assert_eq!(
            publishes,
            vec![
                "Chan:activity".to_owned(),
                "Chan:events=self".to_owned(),
                "Chan:message".to_owned(),
            ],
        );
        assert_eq!(declarations.objects, vec!["Chan", "Plain"]);
    }

    #[test]
    fn receives_entries_and_follow_sites_ride_the_extraction() {
        let declarations = extract(
            files(&[(
                "main.lua",
                r#"
                local Feed = object "Feed" {
                    publishes = { "post" },
                    hooks = {
                        follow = function(state, topic, follower) return true end,
                    },
                }
                local Wall = object "Wall" {
                    receives = {
                        ["Feed:post"] = function(state, event) end,
                    },
                    pin = function(state, id)
                        state:follow(Feed(id), "post")
                    end,
                }
                on "fetch" (function() end)
                "#,
            )]),
            "main.lua",
        )
        .expect("extracts");

        assert_eq!(declarations.receives, vec!["Wall<-Feed:post"]);
        assert_eq!(declarations.follow_sites, vec!["Feed:post"]);
    }

    #[test]
    fn declared_migration_directories_ride_the_contract() {
        let declarations = extract(
            files(&[(
                "main.lua",
                r#"
                local db = database "catalog" { migrations = "migrations/catalog" }
                local scratch = database "notes"
                local Auction = object "Auction" {
                    migrations = "migrations/Auction",
                    bid = function(state) end,
                }
                local Plain = object "Plain" { poke = function(state) end }
                on "fetch" (function() end)
                "#,
            )]),
            "main.lua",
        )
        .expect("extracts");

        let mut databases = declarations.databases.clone();
        databases.sort();
        assert_eq!(
            databases,
            vec!["catalog=migrations/catalog".to_owned(), "notes".to_owned()],
            "a declared directory rides the name; a bare database stays bare"
        );

        let mut objects = declarations.objects.clone();
        objects.sort();
        assert_eq!(
            objects,
            vec!["Auction=migrations/Auction".to_owned(), "Plain".to_owned()],
        );
    }

    #[test]
    fn the_receive_funnel_is_refused() {
        let error = extract(
            files(&[(
                "main.lua",
                r#"
                local Relic = object "Relic" {
                    hooks = {
                        receive = function(state, event) end,
                    },
                }
                on "fetch" (function() end)
                "#,
            )]),
            "main.lua",
        )
        .expect_err("the funnel spelling must refuse");
        assert!(
            error.contains("hooks.receive"),
            "names the spelling: {error}"
        );
    }

    #[test]
    fn malformed_receives_keys_are_refused() {
        let error = extract(
            files(&[(
                "main.lua",
                r#"
                local Wall = object "Wall" {
                    receives = {
                        ["justatopic"] = function(state, event) end,
                    },
                }
                on "fetch" (function() end)
                "#,
            )]),
            "main.lua",
        )
        .expect_err("keys must be Source:topic");
        assert!(error.contains("justatopic"), "names the key: {error}");
    }

    #[test]
    fn declarations_are_recorded_without_running_handlers() {
        let declarations = extract(
            files(&[(
                "main.lua",
                r#"
                local visits = kv "visits"
                local sessions = kv "sessions"
                local token = secret "stripe" .. ""

                on "fetch" (function(request)
                    error("handlers must never run during extraction")
                end)

                local Room = object "Room" {
                    announce = function(state, text)
                        error("methods must never run during extraction")
                    end,
                }
                local Other = objects "Elsewhere"
                local db = database "main"
                local renders = queue "gpu"
                "#,
            )]),
            "main.lua",
        )
        .expect("extraction succeeds");

        assert_eq!(declarations.kv, vec!["visits", "sessions"]);
        assert_eq!(declarations.events, vec!["fetch"]);
        assert_eq!(declarations.secrets, vec!["stripe"]);
        // Declaring records; referencing does not.
        assert_eq!(declarations.objects, vec!["Room"]);
        assert_eq!(declarations.databases, vec!["main"]);
        assert_eq!(declarations.queues, vec!["gpu"]);
    }

    #[test]
    fn lifecycle_declarations_are_recorded() {
        let declarations = extract(
            files(&[(
                "main.lua",
                r#"object "Session" {
                    expire = "30d",
                    admit = function(name) return #name > 3 end,
                }
                on "fetch" (function() end)"#,
            )]),
            "main.lua",
        )
        .expect("extraction succeeds");
        assert_eq!(
            declarations.lifecycle,
            vec!["Session:expire=30d".to_owned(), "Session:admit".to_owned()]
        );
    }

    #[test]
    fn connection_declarations_are_recorded_and_bad_bodies_refuse() {
        let declarations = extract(
            files(&[(
                "main.lua",
                r#"connection "Session" {
                    open = function(conn) end,
                    frame = function(conn, data) end,
                }
                on "fetch" (function() end)"#,
            )]),
            "main.lua",
        )
        .expect("extraction succeeds");
        assert_eq!(declarations.connections, vec!["Session"]);

        let refused = extract(
            files(&[(
                "main.lua",
                r#"connection "Session" { bid = function() end }"#,
            )]),
            "main.lua",
        )
        .expect_err("a method-bearing body must refuse");
        assert!(refused.contains("not a connection handler"), "{refused}");
    }

    #[test]
    fn declarations_in_required_modules_are_recorded() {
        let declarations = extract(
            files(&[
                ("main.lua", r#"require "store" on "fetch" (function() end)"#),
                ("store.lua", r#"local cache = kv "cache" return cache"#),
            ]),
            "main.lua",
        )
        .expect("extraction succeeds");

        assert_eq!(declarations.kv, vec!["cache"]);
    }

    #[test]
    fn ambient_globals_absorb_top_level_use() {
        extract(
            files(&[(
                "main.lua",
                r#"
                local greeting = json.stringify({ hello = "world" })
                log.info(greeting)
                on "fetch" (function() end)
                "#,
            )]),
            "main.lua",
        )
        .expect("stubs absorb platform calls");
    }

    #[test]
    fn a_syntax_error_fails_the_declaration_pass() {
        let error = extract(files(&[("main.lua", "this is ((( not lua")]), "main.lua")
            .expect_err("broken code must fail");
        assert!(error.contains("Declaration pass failed"), "{error}");
    }

    #[test]
    fn a_missing_entry_point_is_reported() {
        let error = extract(files(&[("other.lua", "return 1")]), "main.lua")
            .expect_err("a bundle without its entry point must fail");
        assert!(error.contains("Entry point"), "{error}");
    }

    #[test]
    fn a_bad_cron_expression_fails_at_extraction() {
        let error = extract(
            files(&[("main.lua", r#"on "cron:whenever" (function() end)"#)]),
            "main.lua",
        )
        .expect_err("a bad schedule must fail the pass");
        assert!(error.contains("cron expression"), "{error}");

        extract(
            files(&[("main.lua", r#"on "cron:*/5 * * * *" (function() end)"#)]),
            "main.lua",
        )
        .expect("a real schedule extracts");
    }

    #[test]
    fn a_runaway_top_level_is_interrupted() {
        // The extractor runs untrusted code; a top-level infinite loop must
        // end in an error, not hold the caller hostage.
        let error = extract(files(&[("main.lua", "while true do end")]), "main.lua")
            .expect_err("a runaway loop must be interrupted");

        assert!(error.contains("too long"), "{error}");
    }
}
