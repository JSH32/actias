//! Running one shell chunk: the session vm the query shell escalates to
//! when a statement is more than a read or a call.
//!
//! The chunk is wrapped as the handler of a synthetic revision whose
//! contract is the session's grants, so `kv "users"`, `database "main"`
//! and `objects "Guild"` bind exactly what the operator may open and
//! nothing else, checked by the same `assert_contract_allows` a script
//! is checked by. The vm is the ordinary one (sandboxed, metered,
//! `ALL_SAFE`), the budget is a request's, and object calls go through
//! the router like any script's, so a shell chunk is never a side
//! channel into a file. Prints are captured and returned in order.
use std::sync::Arc;

use actias_worker_core::proto::script_service::{Capabilities, Revision, Script, ScriptConfig};
use actias_worker_core::proto::worker_data::{ShellOutcome, ShellRun};
use actias_worker_core::runtime::{ActiasRuntime, PreparedRevision};
use mlua::LuaSerdeExt;

use crate::routing::ObjectRouting;
use crate::server::AppState;
use actias_worker_core::extensions::objects::{DirectoryLister, ObjectRouter};

/// The most wall time a chunk may take, whatever the request asks.
const MAX_WALL_SECS: u64 = 60;

/// The handler name the chunk registers under; a chunk is a script
/// with one event.
const EVENT: &str = ActiasRuntime::SHELL_EVENT;

fn wrap(source: &str, classes: &[String]) -> String {
    // Every granted class is bound by its own name before the chunk,
    // so `Guild:find { }` reads in a chunk as it reads on the line;
    // a script would write `objects "Guild"` itself. Only names that
    // are identifiers can be bound this way, which every class name is.
    let bindings: String = classes
        .iter()
        .filter(|class| {
            class.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && class
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        })
        .map(|class| format!("    local {class} = objects \"{class}\"\n"))
        .collect();
    // The chunk's own `return` becomes the handler's return, and a
    // chunk that returns nothing yields nil, which is what "null for
    // nothing" means on the wire. The inner function keeps the chunk's
    // locals its own.
    //
    // A bare expression (`Guild:find().entries[1].name`) is not a Luau
    // statement, so it is tried with `return` in front first, the way
    // every repl does; only what does not parse that way runs as
    // written. The check is a parse in a throwaway vm, nothing runs.
    let body = if expression(source) {
        format!("return {source}")
    } else {
        source.to_owned()
    };
    format!(
        "on \"{EVENT}\" (function()\n{bindings}    return (function()\n{body}\n    end)()\nend)\n"
    )
}

/// Whether `source` parses as a single expression.
fn expression(source: &str) -> bool {
    if source.contains('\n') || source.trim_start().starts_with("return ") {
        return false;
    }
    let lua = mlua::Lua::new();
    lua.load(format!("return {source}")).into_function().is_ok()
}

pub async fn run(state: &AppState, run: ShellRun) -> Result<ShellOutcome, String> {
    let wall_secs = run.wall_secs.clamp(1, MAX_WALL_SECS);
    let bound = run
        .objects
        .iter()
        .filter(|class| class.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        .count();
    let script = Script {
        id: "shell".to_owned(),
        project_id: run.scope_id.clone(),
        public_identifier: "__shell".to_owned(),
        ..Default::default()
    };
    let source = wrap(&run.source, &run.objects);
    let revision = Revision {
        id: "shell".to_owned(),
        script_id: "shell".to_owned(),
        script_config: Some(ScriptConfig {
            entry_point: "shell.lua".to_owned(),
            capabilities: Some(Capabilities {
                kv: run.kv,
                databases: run.databases,
                objects: run.objects,
                ..Default::default()
            }),
            ..Default::default()
        }),
        bundle: Some(actias_worker_core::proto::bundle::Bundle {
            entry_point: "shell.lua".to_owned(),
            files: vec![actias_worker_core::proto::bundle::File {
                file_path: "shell.lua".to_owned(),
                content: source.into_bytes(),
                ..Default::default()
            }],
        }),
        ..Default::default()
    };
    let prepared =
        Arc::new(PreparedRevision::prepare(script, revision).map_err(|error| shell_error(&error))?);
    let (publisher, captured) = actias_worker_core::extensions::log::LogPublisher::capturing();
    let runtime = ActiasRuntime::new(
        prepared.clone(),
        state.clients.kv.clone(),
        state.egress.clone(),
        Some(publisher),
        state.secret_client.clone(),
        Some(wall_secs),
    )
    .await
    .map_err(|error| shell_error(&error))?;
    state.guest_limits.apply(&runtime);
    let routing = ObjectRouting::new(state, prepared.clone());
    runtime.set_app_data::<ObjectRouter>(routing.as_router());
    runtime.set_app_data::<DirectoryLister>(routing.as_lister());
    // A shell statement may open a wire too, for trying a provider by
    // hand; the connection outlives the statement like any other.
    runtime.set_app_data::<actias_worker_core::extensions::sockets::Dialer>(
        crate::server::dialer_for(state.clone(), prepared.clone(), None),
    );
    // `print` goes where the log lines go, in the same order, so a
    // chunk's output reads as one transcript.
    {
        let lines = captured.clone();
        let print = runtime
            .create_function(move |lua, values: mlua::Variadic<mlua::Value>| {
                let mut parts = Vec::with_capacity(values.len());
                for value in values {
                    parts.push(match value {
                        mlua::Value::String(text) => text.to_str()?.to_string(),
                        mlua::Value::Nil => "nil".to_owned(),
                        other => match lua.from_value::<serde_json::Value>(other.clone()) {
                            Ok(json) => json.to_string(),
                            Err(_) => format!("{other:?}"),
                        },
                    });
                }
                lines.lock().unwrap_or_else(|p| p.into_inner()).push(
                    actias_common::logging::LogLine {
                        level: "print".to_owned(),
                        message: parts.join("\t"),
                        timestamp_ms: 0,
                    },
                );
                Ok(())
            })
            .map_err(|error| shell_error(&error))?;
        runtime
            .globals()
            .set("print", print)
            .map_err(|error| shell_error(&error))?;
    }
    let listener = runtime
        .listener(EVENT)
        .map_err(|error| shell_error(&error))?;
    runtime.allow_declarations(true);
    // The session's mode travels with the chunk: read-only marks the vm
    // so the writing verbs refuse inside it, and the reads keep working.
    runtime.set_read_only_session(!run.write);
    runtime.begin_call_budget(wall_secs);
    runtime.start_timer();
    let answered: Result<mlua::Value, mlua::Error> = listener.call_async(()).await;
    runtime.end_call_budget();
    let consumed = runtime.consumed();
    let output: Vec<String> = captured
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .map(|line| {
            if line.level == "print" {
                line.message.clone()
            } else {
                format!("[{}] {}", line.level, line.message)
            }
        })
        .collect();
    match answered {
        Ok(value) => {
            let json = match runtime.from_value::<serde_json::Value>(value.clone()) {
                Ok(json) => json,
                // A function or userdata has no json; say what it was.
                Err(_) => serde_json::Value::String(format!("{value:?}")),
            };
            Ok(ShellOutcome {
                value_json: json.to_string(),
                output,
                error: String::new(),
                work: consumed.work,
                wall_ms: 0,
            })
        }
        Err(error) => Ok(ShellOutcome {
            value_json: String::new(),
            output,
            error: shell_error_at(&error, 2 + bound as i64),
            work: consumed.work,
            wall_ms: 0,
        }),
    }
}

/// The chunk's own error text, with the wrapper's lines subtracted so
/// a line number points at what the person typed.
fn shell_error(error: &mlua::Error) -> String {
    shell_error_at(error, 2)
}

/// [`shell_error`] with the wrapper's own line count subtracted, which
/// grows by one per class the wrapper bound.
fn shell_error_at(error: &mlua::Error, wrapper_lines: i64) -> String {
    let mut text = error.to_string();
    // A refusal or a runtime error nests once per async hop and drags
    // the wrapper's traceback along; the person wants the sentence and
    // the line, so the traceback goes and the prefix collapses.
    if let Some(at) = text.find("\nstack traceback:") {
        text.truncate(at);
    }
    while text.starts_with("runtime error: runtime error: ") {
        text.replace_range(0.."runtime error: ".len(), "");
    }
    // The wrapper puts the chunk's first line on line 3 of the entry.
    let re = |s: &str| -> String {
        let mut out = String::new();
        let mut rest = s;
        while let Some(at) = rest.find("shell.lua\":") {
            out.push_str(&rest[..at]);
            let after = &rest[at + "shell.lua\":".len()..];
            let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            match digits.parse::<i64>() {
                Ok(line) => {
                    out.push_str(&format!("line {}", (line - wrapper_lines).max(1)));
                    rest = &after[digits.len()..];
                }
                Err(_) => {
                    out.push_str("shell.lua\":");
                    rest = after;
                }
            }
        }
        out.push_str(rest);
        out
    };
    re(&text)
}
