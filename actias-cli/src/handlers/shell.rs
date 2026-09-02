//! `actias shell <project>`: the query shell, in the terminal.
//!
//! The same shell the console has, with the same three parts. The
//! analyser is the native language service (`actias-luau`, the process
//! `actias lsp` and `actias check` run), asked to complete and check a
//! session document: one typed handle per class synthesized from the
//! contract, every submitted statement verbatim, the current line last.
//! Execution is client-side resolution: a statement is run inside a
//! sandboxed Luau vm whose only globals are the class handles, and the
//! handles record what was asked (a read with its predicate, a call with
//! its arguments) rather than doing it, so the recorded plan is then
//! posted to the same api endpoints the console uses. Anything else in
//! the statement has nothing to run against, so it fails in the sandbox
//! and the shell says what it can run.
//!
//! Write mode is off until `\write`: the shell cannot tell a read from
//! a write, so a read-only session refuses every method call at the
//! call, never by hiding methods.
use std::io::IsTerminal;
use std::sync::{Arc, Mutex};

use colored::*;
use mlua::LuaSerdeExt;
use prettytable::{Table, row};
use serde_json::{Value, json};

use crate::client::Client;
use crate::errors::{Error, Result, progenitor_error};
use crate::service::Service;

/// One class as the contract declares it.
#[derive(Clone)]
struct Klass {
    name: String,
    directory: bool,
    fields: Vec<(String, String)>,
    methods: Vec<String>,
}

/// What the project holds besides classes, for binding and completion.
#[derive(Clone, Default)]
struct Resources {
    namespaces: Vec<String>,
    databases: Vec<String>,
}

/// What a statement asked for, recorded by the sandbox.
#[derive(Clone, Debug)]
enum Plan {
    Read {
        class: String,
        verb: &'static str,
        query: Value,
    },
    Call {
        class: String,
        name: String,
        method: String,
        args: Vec<Value>,
    },
    Kv {
        namespace: String,
        op: String,
        key: Option<String>,
        value: Option<Value>,
    },
    Sql {
        database: String,
        op: String,
        sql: String,
        params: Vec<Value>,
    },
    /// A chunk to run for real, in a session vm on a worker.
    Chunk(String),
}

const HELP: &str = "\
Auction:find { state = \"open\", high_bid = { gt = 100 } }
Auction:list { where = { state = \"open\" }, order = { high_bid = \"desc\" }, limit = 20 }
Auction:visit { where = { state = \"open\" } }     -- every row checked against its object
page = Auction:find { ... }                      -- the name is typed for the next line
kv(\"users\"):get(\"ada\")   kv(\"users\"):list()   kv(\"users\"):set(\"k\", { any = \"json\" })   kv(\"users\"):delete(\"k\")
database(\"main\"):query(\"select * from lots where owner = ?\", { \"ada\" })   database(\"main\"):exec(\"delete from ...\")
Auction(\"lot-42\"):bid(\"ada\", 120)                -- one call on one instance; write mode only
\\write           allow writes this session: set, delete, exec, method calls (logged against you)
\\read            back to read-only
\\run <path>      run a file as a chunk on a worker, loops and all; read-only unless \\write
\\paste           read lines until \\end, then run them as a chunk
\\fields Class    the fields and methods a class exposes
\\resources       the namespaces, databases and classes this shell can bind
\\clear           forget the history
\\quit            leave
One statement resolves here into one request. Anything else runs as a chunk in a fresh vm on a worker under your grants; read-only refuses set, delete, exec and method calls inside it.";

/// The session document the analyser sees: the cli's own prologue (the
/// shipped definitions as typed shadows, exactly what `actias check`
/// uses), one handle per class, the history, the current line. Returns
/// the text and the one-based line the current statement sits on.
fn session_document(classes: &[Klass], history: &[String], current: &str) -> (String, usize) {
    let mut lines: Vec<String> = crate::analyze::prologue(false)
        .lines()
        .map(str::to_owned)
        .collect();
    for klass in classes {
        let mut names = vec![("name".to_owned(), "string".to_owned())];
        names.extend(klass.fields.iter().cloned());
        let union: Vec<String> = names.iter().map(|(n, _)| format!("\"{n}\"")).collect();
        lines.push(format!("type {}Field = {}", klass.name, union.join(" | ")));
        lines.push(format!("type {}Where = {{", klass.name));
        for (name, _) in &names {
            lines.push(format!("    [\"{name}\"]: DirectoryFilter?,"));
        }
        lines.push(format!("    [{}Field]: DirectoryFilter,", klass.name));
        for combinator in ["any", "all", "none"] {
            lines.push(format!("    {combinator}: {{ {}Where }}?,", klass.name));
        }
        lines.push("}".to_owned());
        lines.push(format!("type {}Options = {{", klass.name));
        lines.push(format!("    where: {}Where?,", klass.name));
        lines.push("    order: { [string]: string }?,".to_owned());
        lines.push("    limit: number?,".to_owned());
        lines.push("    cursor: string?,".to_owned());
        lines.push("}".to_owned());
        lines.push(format!("type {}Instance = {{", klass.name));
        for method in &klass.methods {
            lines.push(format!("    {method}: (self: any, ...any) -> any,"));
        }
        lines.push("}".to_owned());
        lines.push(format!("local {}: {{", klass.name));
        if klass.directory {
            lines.push(format!(
                "    list: (self: any, options: {0}Options?) -> DirectoryPage,",
                klass.name
            ));
            lines.push(format!(
                "    find: (self: any, predicate: {0}Where?) -> DirectoryPage,",
                klass.name
            ));
            lines.push(format!(
                "    visit: (self: any, options: {0}Options?) -> DirectoryVisitPage,",
                klass.name
            ));
        }
        lines.push(format!(
            "    get: (self: any, name: string) -> {0}Instance,",
            klass.name
        ));
        lines.push(format!(
            "}} & ((name: string) -> {0}Instance) = nil :: any",
            klass.name
        ));
    }
    lines.extend(history.iter().cloned());
    lines.push(current.to_owned());
    let line = lines.len();
    (lines.join("\n"), line)
}

/// Runs one statement in a sandbox whose only globals are recording
/// class handles, and returns what it asked for.
fn plan(
    statement: &str,
    classes: &[Klass],
    resources: &Resources,
) -> std::result::Result<Plan, String> {
    let lua = mlua::Lua::new();
    lua.sandbox(true).map_err(|e| e.to_string())?;
    let recorded: Arc<Mutex<Vec<Plan>>> = Arc::default();
    let env = lua.create_table().map_err(|e| e.to_string())?;
    // `kv "users"` / `kv("users")` and `database "main"`: handles whose
    // verbs record what was asked, as the class handles do.
    for (family, known) in [
        ("kv", resources.namespaces.clone()),
        ("database", resources.databases.clone()),
    ] {
        let recorded = recorded.clone();
        let binder = lua
            .create_function(move |lua, name: String| {
                if !known.contains(&name) {
                    return Err(mlua::Error::RuntimeError(format!(
                        "{family} '{name}' is not in this project; {family}s: {}",
                        if known.is_empty() {
                            "none".to_owned()
                        } else {
                            known.join(", ")
                        }
                    )));
                }
                let handle = lua.create_table()?;
                let meta = lua.create_table()?;
                let recorded = recorded.clone();
                meta.set(
                    "__index",
                    lua.create_function(move |lua, (_t, op): (mlua::Table, String)| {
                        let name = name.clone();
                        let recorded = recorded.clone();
                        lua.create_function(move |lua, args: mlua::MultiValue| {
                            let mut values = Vec::new();
                            for value in args.into_iter().skip(1) {
                                values.push(lua.from_value::<Value>(value)?);
                            }
                            let planned = if family == "kv" {
                                let key = values.first().and_then(Value::as_str).map(str::to_owned);
                                Plan::Kv {
                                    namespace: name.clone(),
                                    op: op.clone(),
                                    key,
                                    value: values.get(1).cloned(),
                                }
                            } else {
                                let sql = values
                                    .first()
                                    .and_then(Value::as_str)
                                    .map(str::to_owned)
                                    .unwrap_or_default();
                                let params = values
                                    .get(1)
                                    .and_then(Value::as_array)
                                    .cloned()
                                    .unwrap_or_default();
                                Plan::Sql {
                                    database: name.clone(),
                                    op: op.clone(),
                                    sql,
                                    params,
                                }
                            };
                            recorded
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .push(planned);
                            Ok(mlua::Value::Nil)
                        })
                    })?,
                )?;
                handle.set_metatable(Some(meta))?;
                Ok(handle)
            })
            .map_err(|e| e.to_string())?;
        env.set(family, binder).map_err(|e| e.to_string())?;
    }
    for klass in classes {
        let handle = lua.create_table().map_err(|e| e.to_string())?;
        for verb in ["list", "find", "visit"] {
            let class = klass.name.clone();
            let recorded = recorded.clone();
            let reader = lua
                .create_function(
                    move |lua, (_this, options): (mlua::Table, Option<mlua::Value>)| {
                        let query: Value = match options {
                            Some(value) => lua.from_value(value)?,
                            None => json!({}),
                        };
                        recorded
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .push(Plan::Read {
                                class: class.clone(),
                                verb,
                                query,
                            });
                        lua.create_table()
                    },
                )
                .map_err(|e| e.to_string())?;
            handle.set(verb, reader).map_err(|e| e.to_string())?;
        }
        // `Class("name")` and `Class:get("name")` both yield an instance
        // whose every method records the call.
        let instance_of = {
            let class = klass.name.clone();
            let recorded = recorded.clone();
            lua.create_function(move |lua, name: String| {
                let instance = lua.create_table()?;
                let meta = lua.create_table()?;
                let class = class.clone();
                let recorded = recorded.clone();
                meta.set(
                    "__index",
                    lua.create_function(move |lua, (_t, method): (mlua::Table, String)| {
                        let class = class.clone();
                        let name = name.clone();
                        let recorded = recorded.clone();
                        lua.create_function(move |lua, args: mlua::MultiValue| {
                            let mut values = Vec::new();
                            // The first value is the instance itself (`:`).
                            for value in args.into_iter().skip(1) {
                                values.push(lua.from_value::<Value>(value)?);
                            }
                            recorded
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .push(Plan::Call {
                                    class: class.clone(),
                                    name: name.clone(),
                                    method: method.clone(),
                                    args: values,
                                });
                            Ok(mlua::Value::Nil)
                        })
                    })?,
                )?;
                instance.set_metatable(Some(meta))?;
                Ok(instance)
            })
            .map_err(|e| e.to_string())?
        };
        handle
            .set(
                "get",
                lua.create_function({
                    let instance_of = instance_of.clone();
                    move |_, (_this, name): (mlua::Table, String)| {
                        instance_of.call::<mlua::Table>(name)
                    }
                })
                .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
        let meta = lua.create_table().map_err(|e| e.to_string())?;
        meta.set(
            "__call",
            lua.create_function(move |_, (_this, name): (mlua::Table, String)| {
                instance_of.call::<mlua::Table>(name)
            })
            .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        handle
            .set_metatable(Some(meta))
            .map_err(|e| e.to_string())?;
        env.set(klass.name.as_str(), handle)
            .map_err(|e| e.to_string())?;
    }
    let chunk = lua.load(statement).set_environment(env);
    let outcome = chunk.exec();
    let mut planned = std::mem::take(&mut *recorded.lock().unwrap_or_else(|p| p.into_inner()));
    let sentence = "one statement: Class:find/list/visit { ... }, kv(\"ns\"):get/list/set/delete(...), database(\"db\"):query/exec(...), or in write mode Class(\"name\"):method(...); a loop or an expression is a chunk, which \\run and \\paste take";
    match (planned.len(), outcome) {
        // Exactly one ask and nothing went wrong after it: that is one
        // statement. An ask followed by an error (`Guild:get(Guild:find()
        // .entries[1].name)`: the inner find recorded, then the outer
        // indexed a result the sandbox never had) is not.
        (1, Ok(())) => Ok(planned.pop().unwrap_or(Plan::Chunk(statement.to_owned()))),
        (0, Err(error)) => {
            let text = error.to_string();
            // A message the sandbox wrote on purpose (a class or a
            // resource the project does not hold) is the answer; a
            // Luau error about anything else means the line was not one
            // statement.
            let known = text.contains("not in this project")
                || classes.iter().any(|k| text.contains(&k.name));
            Err(if known { text } else { sentence.to_owned() })
        }
        (0, Ok(())) => Err(
            "that ran but asked for nothing; one statement is a read, a kv or database verb, or in write mode a method call; a chunk is what \\run and \\paste take".to_owned(),
        ),
        _ => Err(sentence.to_owned()),
    }
}

const OPERATORS: [&str; 10] = [
    "eq",
    "ne",
    "lt",
    "lte",
    "gt",
    "gte",
    "one_of",
    "starts_with",
    "contains",
    "exists",
];

/// The wire's predicate tree for one where-table, as the console's
/// reader builds it: a bare value is equality, a table under a field
/// is operators, `any`/`all`/`none` take lists of where-tables.
fn where_to_wire(table: &Value) -> std::result::Result<Value, String> {
    let Some(entries) = table.as_object() else {
        return Err("a where is a table of field = value".to_owned());
    };
    let mut conditions = Vec::new();
    let mut wire = serde_json::Map::new();
    for (key, value) in entries {
        if ["any", "all", "none"].contains(&key.as_str()) {
            let Some(branches) = value.as_array() else {
                return Err(format!("'{key}' takes a list of filters"));
            };
            let translated = branches
                .iter()
                .map(where_to_wire)
                .collect::<std::result::Result<Vec<_>, _>>()?;
            wire.insert(key.clone(), Value::Array(translated));
            continue;
        }
        match value {
            Value::Object(operators) => {
                for (op, operand) in operators {
                    if !OPERATORS.contains(&op.as_str()) {
                        return Err(format!("'{op}' is not an operator"));
                    }
                    conditions.push(json!({
                        "field": key,
                        "op": op,
                        "valueJson": operand.to_string(),
                    }));
                }
            }
            other => conditions.push(json!({
                "field": key,
                "op": "eq",
                "valueJson": other.to_string(),
            })),
        }
    }
    wire.insert("conditions".to_owned(), Value::Array(conditions));
    Ok(Value::Object(wire))
}

/// The wire's query for what a read asked: `find` takes the predicate
/// alone; `list` and `visit` take the option table.
fn query_to_wire(verb: &str, asked: &Value) -> std::result::Result<Value, String> {
    if verb == "find" {
        return Ok(json!({ "where": where_to_wire(asked)? }));
    }
    let mut wire = serde_json::Map::new();
    if let Some(options) = asked.as_object() {
        for key in options.keys() {
            if !["where", "order", "limit", "cursor"].contains(&key.as_str()) {
                return Err(format!(
                    "'{key}' is not an option; list and visit take where, order, limit and cursor"
                ));
            }
        }
        if let Some(where_) = options.get("where") {
            wire.insert("where".to_owned(), where_to_wire(where_)?);
        }
        if let Some(order) = options.get("order") {
            let Some(fields) = order.as_object() else {
                return Err("order is a table of field = \"asc\" | \"desc\"".to_owned());
            };
            let mut list = Vec::new();
            for (field, direction) in fields {
                match direction.as_str() {
                    Some("asc") => list.push(json!({ "field": field, "descending": false })),
                    Some("desc") => list.push(json!({ "field": field, "descending": true })),
                    _ => return Err(format!("order.{field} is \"asc\" or \"desc\"")),
                }
            }
            wire.insert("order".to_owned(), Value::Array(list));
        }
        if let Some(limit) = options.get("limit") {
            if !limit.is_number() {
                return Err("limit is a number".to_owned());
            }
            wire.insert("limit".to_owned(), limit.clone());
        }
        if let Some(cursor) = options.get("cursor") {
            if !cursor.is_string() {
                return Err("cursor is a string".to_owned());
            }
            wire.insert("cursor".to_owned(), cursor.clone());
        }
    }
    Ok(Value::Object(wire))
}

/// The api's own sentence for a refused statement, not the transport's
/// description of the response: a 400 carries the refusal a caller
/// should read ("'titel' is not a directory field of this class"), and
/// the generated client only knows it as an unexpected status.
async fn explained<T: std::fmt::Debug>(error: crate::client::Error<T>) -> Error {
    match error {
        crate::client::Error::UnexpectedResponse(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let message = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|value| {
                    value
                        .get("message")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or(body);
            Error::Api(if status.is_client_error() {
                message
            } else {
                format!("{status}: {message}")
            })
        }
        other => progenitor_error(other),
    }
}

/// A page's entries as a table: name, then the union of the fields.
fn render_page(entries: &[Value], verified: bool) {
    if entries.is_empty() {
        println!("  {}", "no rows".dimmed());
        return;
    }
    let mut columns: Vec<String> = Vec::new();
    for entry in entries {
        let fields = entry
            .get("entry")
            .unwrap_or(entry)
            .get("fields")
            .and_then(Value::as_object);
        if let Some(fields) = fields {
            for key in fields.keys() {
                if !columns.contains(key) {
                    columns.push(key.clone());
                }
            }
        }
    }
    columns.sort();
    let mut table = Table::new();
    let mut head = row![b -> "name"];
    for column in &columns {
        head.add_cell(prettytable::Cell::new(column).style_spec("b"));
    }
    if verified {
        head.add_cell(prettytable::Cell::new("checked").style_spec("b"));
    }
    table.add_row(head);
    for entry in entries {
        let inner = entry.get("entry").unwrap_or(entry);
        let name = inner.get("name").and_then(Value::as_str).unwrap_or("");
        let mut line = row![name];
        for column in &columns {
            let cell = inner
                .get("fields")
                .and_then(|f| f.get(column))
                .and_then(Value::as_str)
                .map(|raw| {
                    serde_json::from_str::<Value>(raw)
                        .map(|v| match v {
                            Value::String(s) => s,
                            other => other.to_string(),
                        })
                        .unwrap_or_else(|_| raw.to_owned())
                })
                .unwrap_or_else(|| "-".to_owned());
            line.add_cell(prettytable::Cell::new(&cell));
        }
        if verified {
            let unverified = entry
                .get("unverified")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            line.add_cell(prettytable::Cell::new(if unverified {
                "unverified"
            } else {
                "yes"
            }));
        }
        table.add_row(line);
    }
    table.printstd();
}

async fn run_plan(client: &Client, project: &str, write: bool, plan: Plan) -> Result<()> {
    let started = std::time::Instant::now();
    match plan {
        Plan::Read { class, verb, query } => {
            let wire = query_to_wire(verb, &query).map_err(Error::Command)?;
            let body: crate::client::types::DirectoryQueryDto = serde_json::from_value(wire)
                .map_err(|e| Error::Command(format!("the query does not fit: {e}")))?;
            let (entries, cursor, building, verified): (
                Vec<Value>,
                Option<String>,
                Vec<String>,
                bool,
            ) = if verb == "visit" {
                let page = client
                    .object_directory_visit()
                    .project(project)
                    .class(&class)
                    .body(body)
                    .send()
                    .await;
                let page = match page {
                    Ok(answer) => answer.into_inner(),
                    Err(error) => return Err(explained(error).await),
                };
                let raw = serde_json::to_value(&page).unwrap_or(Value::Null);
                (
                    raw.get("entries")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default(),
                    page.cursor,
                    page.building,
                    true,
                )
            } else {
                let page = client
                    .object_directory()
                    .project(project)
                    .class(&class)
                    .body(body)
                    .send()
                    .await;
                let page = match page {
                    Ok(answer) => answer.into_inner(),
                    Err(error) => return Err(explained(error).await),
                };
                let raw = serde_json::to_value(&page).unwrap_or(Value::Null);
                (
                    raw.get("entries")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default(),
                    page.cursor,
                    page.building,
                    false,
                )
            };
            let mut summary = format!(
                "{} row{}",
                entries.len(),
                if entries.len() == 1 { "" } else { "s" }
            );
            if cursor.is_some() {
                summary.push_str(", more after a cursor");
            }
            if !building.is_empty() {
                summary.push_str(&format!(", building: {}", building.join(", ")));
            }
            summary.push_str(&format!(" · {} ms", started.elapsed().as_millis()));
            if verified {
                summary.push_str(" · every row checked against its object");
            }
            println!("  {}", summary.dimmed());
            render_page(&entries, verified);
        }
        Plan::Call {
            class,
            name,
            method,
            args,
        } => {
            let body = crate::client::types::ObjectCallDto { method, args };
            let result = client
                .object_call()
                .project(project)
                .class(&class)
                .name(&name)
                .body(body)
                .send()
                .await;
            let result = match result {
                Ok(result) => result.into_inner(),
                Err(error) => return Err(explained(error).await),
            };
            let value: Value = serde_json::from_str(&result.value_json)
                .unwrap_or(Value::String(result.value_json.clone()));
            println!(
                "  {}",
                format!("returned · {} ms", started.elapsed().as_millis()).dimmed()
            );
            println!(
                "{}",
                serde_json::to_string_pretty(&value).unwrap_or_default()
            );
        }
        Plan::Kv {
            namespace,
            op,
            key,
            value,
        } => match op.as_str() {
            "get" => {
                let key = key.ok_or_else(|| Error::Command("kv:get takes a key".to_owned()))?;
                let pair = client
                    .get_key()
                    .project(project)
                    .namespace(&namespace)
                    .key(&key)
                    .send()
                    .await;
                let pair = match pair {
                    Ok(pair) => pair.into_inner(),
                    Err(error) => return Err(explained(error).await),
                };
                let kind = format!("{:?}", pair.type_);
                println!(
                    "  {}",
                    format!("{kind} · {} ms", started.elapsed().as_millis()).dimmed()
                );
                println!("{}", pair.value);
            }
            "list" => {
                let page = client
                    .list_namespace()
                    .project(project)
                    .namespace(&namespace)
                    .send()
                    .await;
                let page = match page {
                    Ok(page) => page.into_inner(),
                    Err(error) => return Err(explained(error).await),
                };
                println!(
                    "  {}",
                    format!(
                        "{} pair{}{} · {} ms",
                        page.pairs.len(),
                        if page.pairs.len() == 1 { "" } else { "s" },
                        if page.token.is_some() {
                            ", more after a token"
                        } else {
                            ""
                        },
                        started.elapsed().as_millis()
                    )
                    .dimmed()
                );
                let mut table = Table::new();
                table.add_row(row![b -> "key", b -> "type", b -> "value"]);
                for pair in &page.pairs {
                    let value: String = pair.value.chars().take(80).collect();
                    table.add_row(row![pair.key, format!("{:?}", pair.type_), value]);
                }
                if !page.pairs.is_empty() {
                    table.printstd();
                }
            }
            "set" => {
                let key = key.ok_or_else(|| Error::Command("kv:set takes a key".to_owned()))?;
                let value = value
                    .ok_or_else(|| Error::Command("kv:set takes a key and a value".to_owned()))?;
                let (kind, text) = kv_typed(&value);
                let body: crate::client::types::SetKeyDto = serde_json::from_value(json!({
                    "type": kind,
                    "value": text,
                }))
                .map_err(|e| Error::Command(e.to_string()))?;
                let done = client
                    .set_key()
                    .project(project)
                    .namespace(&namespace)
                    .key(&key)
                    .body(body)
                    .send()
                    .await;
                if let Err(error) = done {
                    return Err(explained(error).await);
                }
                println!(
                    "  {}",
                    format!(
                        "set {namespace}/{key} as {kind} · {} ms",
                        started.elapsed().as_millis()
                    )
                    .dimmed()
                );
            }
            "delete" => {
                let key = key.ok_or_else(|| Error::Command("kv:delete takes a key".to_owned()))?;
                let done = client
                    .delete_key()
                    .project(project)
                    .namespace(&namespace)
                    .key(&key)
                    .send()
                    .await;
                if let Err(error) = done {
                    return Err(explained(error).await);
                }
                println!(
                    "  {}",
                    format!(
                        "deleted {namespace}/{key} · {} ms",
                        started.elapsed().as_millis()
                    )
                    .dimmed()
                );
            }
            other => {
                return Err(Error::Command(format!(
                    "kv takes get, list, set and delete; '{other}' is not one"
                )));
            }
        },
        Plan::Sql {
            database,
            op,
            sql,
            params,
        } => {
            let body: crate::client::types::SqlQueryDto =
                serde_json::from_value(json!({ "sql": sql, "params": params }))
                    .map_err(|e| Error::Command(e.to_string()))?;
            let rows = match op.as_str() {
                "query" => {
                    let answer = client
                        .query()
                        .project(project)
                        .name(&database)
                        .body(body)
                        .send()
                        .await;
                    match answer {
                        Ok(answer) => answer.into_inner().rows,
                        Err(error) => return Err(explained(error).await),
                    }
                }
                "exec" | "execute" => {
                    let answer = client
                        .execute()
                        .project(project)
                        .name(&database)
                        .body(body)
                        .send()
                        .await;
                    match answer {
                        Ok(answer) => answer.into_inner().rows,
                        Err(error) => return Err(explained(error).await),
                    }
                }
                other => {
                    return Err(Error::Command(format!(
                        "database takes query and exec; '{other}' is not one"
                    )));
                }
            };
            println!(
                "  {}",
                format!(
                    "{} row{} · {} ms",
                    rows.len(),
                    if rows.len() == 1 { "" } else { "s" },
                    started.elapsed().as_millis()
                )
                .dimmed()
            );
            render_sql(&rows);
        }
        Plan::Chunk(source) => {
            let body: crate::client::types::ShellRunDto =
                serde_json::from_value(json!({ "source": source, "write": write }))
                    .map_err(|e| Error::Command(e.to_string()))?;
            let answer = client.run_shell().project(project).body(body).send().await;
            let outcome = match answer {
                Ok(answer) => answer.into_inner(),
                Err(error) => return Err(explained(error).await),
            };
            for line in &outcome.output {
                println!("{line}");
            }
            if let Some(error) = outcome.error.as_deref().filter(|e| !e.is_empty()) {
                println!("{}", error.red());
            } else {
                let value: Value = serde_json::from_str(&outcome.value_json)
                    .unwrap_or(Value::String(outcome.value_json.clone()));
                println!(
                    "  {}",
                    format!(
                        "returned · {} ms on the node, {} work",
                        outcome.wall_ms as u64, outcome.work as u64
                    )
                    .dimmed()
                );
                if !value.is_null() {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&value).unwrap_or_default()
                    );
                }
            }
        }
    }
    Ok(())
}

/// A kv value's pair type and text, from the literal's json shape.
fn kv_typed(value: &Value) -> (&'static str, String) {
    match value {
        Value::String(text) => ("STRING", text.clone()),
        Value::Bool(flag) => ("BOOLEAN", flag.to_string()),
        Value::Number(number) if number.is_i64() || number.is_u64() => {
            ("INTEGER", number.to_string())
        }
        Value::Number(number) => ("NUMBER", number.to_string()),
        other => ("JSON", other.to_string()),
    }
}

/// Rows as a table, columns from the keys the rows carry.
fn render_sql(rows: &[Value]) {
    if rows.is_empty() {
        return;
    }
    let mut columns: Vec<String> = Vec::new();
    for row in rows {
        if let Some(object) = row.as_object() {
            for key in object.keys() {
                if !columns.contains(key) {
                    columns.push(key.clone());
                }
            }
        }
    }
    let mut table = Table::new();
    let mut head = prettytable::Row::empty();
    for column in &columns {
        head.add_cell(prettytable::Cell::new(column).style_spec("b"));
    }
    table.add_row(head);
    for row in rows {
        let mut line = prettytable::Row::empty();
        for column in &columns {
            let cell = row
                .get(column)
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    Value::Null => "-".to_owned(),
                    other => other.to_string(),
                })
                .unwrap_or_else(|| "-".to_owned());
            line.add_cell(prettytable::Cell::new(&cell));
        }
        table.add_row(line);
    }
    table.printstd();
}

/// The completer: the analyser over the session document, and, when
/// no language service is on this machine, the names the contract
/// declares. Both are "resource aware"; only one of them knows types.
struct Completer {
    classes: Vec<Klass>,
    resources: Resources,
    history: Arc<Mutex<Vec<String>>>,
    service: Option<Mutex<Service>>,
}

impl Completer {
    fn complete_at(&self, line: &str, pos: usize) -> (usize, Vec<String>) {
        let upto = &line[..pos];
        let start = upto
            .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
            .map(|i| i + 1)
            .unwrap_or(0);
        let tail = &upto[start..];
        if let Some(service) = &self.service
            && let Ok(mut service) = service.lock()
        {
            let history = self
                .history
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            let (document, at) = session_document(&self.classes, &history, line);
            if service.set_file("shell.lua", &document).is_ok()
                && let Ok(answer) = service.at("complete", "shell.lua", at, pos + 1)
                && let Some(entries) = answer.as_array()
            {
                let names: Vec<String> = entries
                    .iter()
                    .filter_map(|e| e.get("label").and_then(Value::as_str))
                    .filter(|n| n.starts_with(tail) && *n != tail)
                    .map(str::to_owned)
                    .take(20)
                    .collect();
                if !names.is_empty() {
                    return (start, names);
                }
            }
        }
        // Inside `kv "` / `kv("` or `database "`: the names the project
        // holds, which no type can enumerate.
        let before = upto.trim_end_matches(|c: char| c.is_alphanumeric() || c == '_' || c == '-');
        for (family, known) in [
            ("kv", &self.resources.namespaces),
            ("database", &self.resources.databases),
        ] {
            let opens = [
                format!("{family} \""),
                format!("{family}(\""),
                format!("{family}('"),
                format!("{family} '"),
            ];
            if opens.iter().any(|open| before.ends_with(open.as_str())) {
                let tail = &upto[before.len()..];
                return (
                    before.len(),
                    known
                        .iter()
                        .filter(|n| n.starts_with(tail) && n.as_str() != tail)
                        .cloned()
                        .collect(),
                );
            }
        }
        // The fallback: what the contract says exists.
        let mut names: Vec<String> = Vec::new();
        for klass in &self.classes {
            names.push(klass.name.clone());
            if klass.directory {
                names.extend(["list", "find", "visit"].map(str::to_owned));
                names.extend(klass.fields.iter().map(|(n, _)| n.clone()));
            }
            names.push("get".to_owned());
            names.extend(klass.methods.iter().cloned());
        }
        names.extend(
            [
                "where",
                "order",
                "limit",
                "cursor",
                "any",
                "all",
                "none",
                "eq",
                "ne",
                "lt",
                "lte",
                "gt",
                "gte",
                "one_of",
                "starts_with",
                "contains",
                "exists",
                "kv",
                "database",
                "query",
                "exec",
                "set",
                "delete",
            ]
            .map(str::to_owned),
        );
        names.sort();
        names.dedup();
        (
            start,
            names
                .into_iter()
                .filter(|n| n.starts_with(tail) && n != tail)
                .collect(),
        )
    }
}

#[derive(rustyline::Helper, rustyline::Hinter, rustyline::Highlighter, rustyline::Validator)]
struct ShellHelper(Completer);

impl rustyline::completion::Completer for ShellHelper {
    type Candidate = rustyline::completion::Pair;
    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &rustyline::Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Self::Candidate>)> {
        let (start, names) = self.0.complete_at(line, pos);
        Ok((
            start,
            names
                .into_iter()
                .map(|name| rustyline::completion::Pair {
                    display: name.clone(),
                    replacement: name,
                })
                .collect(),
        ))
    }
}

pub async fn handle(client: &Client, project: &str) -> Result<()> {
    let counted = client
        .count_objects()
        .project(project)
        .send()
        .await
        .map_err(progenitor_error)?
        .into_inner();
    let classes: Vec<Klass> = counted
        .iter()
        .map(|row| Klass {
            name: row.class.clone(),
            directory: row.has_directory,
            fields: row
                .directory_fields
                .iter()
                .map(|f| (f.name.clone(), f.kind.clone()))
                .collect(),
            methods: row.methods.clone(),
        })
        .collect();
    let namespaces = client
        .list_namespaces()
        .project(project)
        .send()
        .await
        .map(|answer| {
            answer
                .into_inner()
                .iter()
                .map(|n| n.name.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let databases = client
        .list_databases()
        .project(project)
        .send()
        .await
        .map(|answer| {
            answer
                .into_inner()
                .iter()
                .map(|d| d.name.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let resources = Resources {
        namespaces,
        databases,
    };
    let history: Arc<Mutex<Vec<String>>> = Arc::default();
    let service = crate::service::locate().and_then(|path| Service::start(&path).ok());
    let typed = service.is_some();
    let completer = Completer {
        classes: classes.clone(),
        resources: resources.clone(),
        history: history.clone(),
        service: service.map(Mutex::new),
    };
    let mut write = false;
    let with_directory: Vec<&str> = classes
        .iter()
        .filter(|k| k.directory)
        .map(|k| k.name.as_str())
        .collect();
    println!(
        "{} {} · {} · {}",
        "actias shell".bold(),
        project,
        if typed {
            "typed completions from the language service"
        } else {
            "completions from the contract (no language service found beside the cli)"
        },
        "read-only".yellow()
    );
    println!(
        "classes with a directory: {}; kv: {}; databases: {}; \\help for the rest",
        if with_directory.is_empty() {
            "none".to_owned()
        } else {
            with_directory.join(", ")
        },
        if resources.namespaces.is_empty() {
            "none".to_owned()
        } else {
            resources.namespaces.join(", ")
        },
        if resources.databases.is_empty() {
            "none".to_owned()
        } else {
            resources.databases.join(", ")
        }
    );

    let interactive = std::io::stdin().is_terminal();
    let mut editor: Option<rustyline::Editor<ShellHelper, rustyline::history::DefaultHistory>> =
        if interactive {
            let mut editor = rustyline::Editor::new()
                .map_err(|e| Error::Command(format!("the terminal could not be opened: {e}")))?;
            editor.set_helper(Some(ShellHelper(completer)));
            Some(editor)
        } else {
            None
        };
    let stdin = std::io::stdin();
    // A chunk being pasted, gathered until `\\end`.
    let mut pasting: Option<Vec<String>> = None;
    loop {
        let line = match editor.as_mut() {
            Some(editor) => match editor.readline(&format!("{} ", ">".cyan())) {
                Ok(line) => line,
                Err(rustyline::error::ReadlineError::Interrupted)
                | Err(rustyline::error::ReadlineError::Eof) => break,
                Err(error) => return Err(Error::Command(error.to_string())),
            },
            None => {
                let mut line = String::new();
                if stdin
                    .read_line(&mut line)
                    .map_err(|e| Error::Io(e.to_string()))?
                    == 0
                {
                    break;
                }
                println!("> {}", line.trim_end());
                line
            }
        };
        let text = line.trim();
        if let Some(buffer) = pasting.as_mut() {
            if text == "\\end" {
                let source = buffer.join("\n");
                pasting = None;
                if let Err(error) = run_plan(client, project, write, Plan::Chunk(source)).await {
                    println!("{}", error.to_string().red());
                }
            } else {
                buffer.push(line.trim_end_matches(['\n', '\r']).to_owned());
            }
            continue;
        }
        if text.is_empty() || text.starts_with("--") {
            continue;
        }
        if let Some(editor) = editor.as_mut() {
            let _ = editor.add_history_entry(text);
        }
        if let Some(command) = text.strip_prefix('\\') {
            let mut words = command.split_whitespace();
            match words.next().unwrap_or("") {
                "help" => println!("{HELP}"),
                "quit" | "exit" | "q" => break,
                "clear" => history.lock().unwrap_or_else(|p| p.into_inner()).clear(),
                "write" => {
                    write = true;
                    println!(
                        "{}",
                        "write mode on for this session: a method call goes through the object's own lane, exactly as a script's would, and is logged against your account. Naming an instance that does not exist creates it, admission permitting."
                            .yellow()
                    );
                }
                "read" => {
                    write = false;
                    println!("write mode off.");
                }
                "run" => {
                    let path = words.next().unwrap_or("");
                    if path.is_empty() {
                        println!("{}", "\\run takes a path".red());
                        continue;
                    }
                    match std::fs::read_to_string(path) {
                        Ok(source) => {
                            if let Err(error) =
                                run_plan(client, project, write, Plan::Chunk(source)).await
                            {
                                println!("{}", error.to_string().red());
                            }
                        }
                        Err(error) => println!("{}", format!("{path}: {error}").red()),
                    }
                }
                "paste" => {
                    println!("paste the chunk, then a line with \\end");
                    pasting = Some(Vec::new());
                }
                "resources" => {
                    println!(
                        "classes: {}",
                        classes
                            .iter()
                            .map(|k| k.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                    println!("kv namespaces: {}", resources.namespaces.join(", "));
                    println!("databases: {}", resources.databases.join(", "));
                }
                "fields" => {
                    let wanted = words.next().unwrap_or("");
                    match classes.iter().find(|k| k.name == wanted) {
                        Some(klass) => {
                            if klass.directory {
                                println!("directory fields:");
                                println!("  name: string (the object's own name)");
                                for (name, kind) in &klass.fields {
                                    println!("  {name}: {kind}");
                                }
                            } else {
                                println!("no directory: list, find and visit are not available");
                            }
                            println!("methods:");
                            if klass.methods.is_empty() {
                                println!("  none declared");
                            }
                            for method in &klass.methods {
                                println!("  {method}(...)");
                            }
                        }
                        None => println!(
                            "{}",
                            format!(
                                "\\fields takes a class: {}",
                                classes
                                    .iter()
                                    .map(|k| k.name.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )
                            .red()
                        ),
                    }
                }
                other => println!(
                    "{}",
                    format!("'\\{other}' is not a command; \\help lists them").red()
                ),
            }
            continue;
        }
        let planned = match plan(text, &classes, &resources) {
            Ok(plan) => plan,
            // Not one statement (an expression, a nested call, a loop):
            // in write mode it runs as a chunk, which is what the person
            // meant; read-only says how to get there.
            Err(message) if !message.contains("not in this project") => {
                println!(
                    "{}",
                    if write {
                        "not one statement; running it as a chunk"
                    } else {
                        "not one statement; running it as a read-only chunk"
                    }
                    .dimmed()
                );
                Plan::Chunk(text.to_owned())
            }
            Err(message) => {
                println!("{}", message.red());
                continue;
            }
        };
        match &planned {
            Plan::Read { class, .. }
                if !classes.iter().any(|k| k.name == *class && k.directory) =>
            {
                println!(
                    "{}",
                    format!("'{class}' declares no directory, so it has nothing to list").red()
                );
                continue;
            }
            Plan::Call { .. } if !write => {
                println!(
                    "{}",
                    "this session is read-only; \\write allows method calls (they run for real, through the object's own lane, and are logged against your account)".red()
                );
                continue;
            }
            Plan::Kv { op, .. } if !write && (op == "set" || op == "delete") => {
                println!(
                    "{}",
                    "this session is read-only; \\write allows kv:set and kv:delete".red()
                );
                continue;
            }
            Plan::Sql { op, .. } if !write && op != "query" => {
                println!(
                    "{}",
                    "this session is read-only; \\write allows database:exec".red()
                );
                continue;
            }
            _ => {}
        }
        match run_plan(client, project, write, planned).await {
            Ok(()) => history
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(text.to_owned()),
            Err(error) => println!("{}", error.to_string().red()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classes() -> Vec<Klass> {
        vec![
            Klass {
                name: "Guild".to_owned(),
                directory: true,
                fields: vec![("title".to_owned(), "string".to_owned())],
                methods: vec!["is_member".to_owned(), "rename".to_owned()],
            },
            Klass {
                name: "Ledger".to_owned(),
                directory: false,
                fields: vec![],
                methods: vec!["append".to_owned()],
            },
        ]
    }

    #[test]
    fn a_read_is_planned_from_the_sandbox() {
        let planned = plan(
            r#"Guild:find { title = "x", public = { eq = true } }"#,
            &classes(),
            &Resources::default(),
        )
        .expect("plans");
        let Plan::Read { class, verb, query } = planned else {
            panic!("a read");
        };
        assert_eq!(class, "Guild");
        assert_eq!(verb, "find");
        let wire = query_to_wire(verb, &query).expect("translates");
        let conditions = wire["where"]["conditions"].as_array().expect("conditions");
        assert_eq!(conditions.len(), 2);
        assert!(
            conditions
                .iter()
                .any(|c| c["field"] == "title" && c["op"] == "eq" && c["valueJson"] == "\"x\"")
        );
        assert!(
            conditions
                .iter()
                .any(|c| c["field"] == "public" && c["op"] == "eq" && c["valueJson"] == "true")
        );
    }

    #[test]
    fn a_list_keeps_its_options_and_an_assignment_is_fine() {
        let planned = plan(
            r#"page = Guild:list { where = { title = "x" }, order = { title = "desc" }, limit = 5 }"#,
            &classes(),
            &Resources::default(),
        )
        .expect("plans");
        let Plan::Read { verb, query, .. } = planned else {
            panic!("a read");
        };
        assert_eq!(verb, "list");
        let wire = query_to_wire(verb, &query).expect("translates");
        assert_eq!(wire["limit"], 5);
        assert_eq!(wire["order"][0]["field"], "title");
        assert_eq!(wire["order"][0]["descending"], true);
        assert_eq!(wire["where"]["conditions"][0]["field"], "title");
    }

    #[test]
    fn a_call_records_its_instance_method_and_literal_arguments() {
        for text in [
            r#"Guild("g-1"):rename("hall", 3, { deep = true })"#,
            r#"Guild:get("g-1"):rename("hall", 3, { deep = true })"#,
        ] {
            let Plan::Call {
                class,
                name,
                method,
                args,
            } = plan(text, &classes(), &Resources::default()).expect("plans")
            else {
                panic!("a call");
            };
            assert_eq!(
                (class.as_str(), name.as_str(), method.as_str()),
                ("Guild", "g-1", "rename")
            );
            assert_eq!(args, vec![json!("hall"), json!(3), json!({ "deep": true })]);
        }
    }

    #[test]
    fn anything_else_is_refused_with_the_sentence() {
        let error = plan(
            "for i = 1, 3 do print(i) end",
            &classes(),
            &Resources::default(),
        )
        .expect_err("refused");
        assert!(error.contains("a chunk"), "{error}");
        let error = plan("Nope:find {}", &classes(), &Resources::default()).expect_err("refused");
        assert!(error.contains("a chunk"), "{error}");
    }

    #[test]
    fn the_fallback_completer_knows_the_contract() {
        let completer = Completer {
            classes: classes(),
            resources: Resources {
                namespaces: vec!["users".to_owned(), "sessions".to_owned()],
                databases: vec!["main".to_owned()],
            },
            history: Arc::default(),
            service: None,
        };
        let (start, names) = completer.complete_at("Gu", 2);
        assert_eq!(start, 0);
        assert_eq!(names, vec!["Guild".to_owned()]);
        let (start, names) = completer.complete_at("Guild:fi", 8);
        assert_eq!(start, 6);
        assert_eq!(names, vec!["find".to_owned()]);
        let (_, names) = completer.complete_at(r#"Guild("g"):ren"#, 14);
        assert_eq!(names, vec!["rename".to_owned()]);
        let (_, names) = completer.complete_at("Guild:find { tit", 16);
        assert_eq!(names, vec!["title".to_owned()]);
        let (start, names) = completer.complete_at(r#"kv("se"#, 6);
        assert_eq!(start, 4);
        assert_eq!(names, vec!["sessions".to_owned()]);
        let (_, names) = completer.complete_at(r#"database "m"#, 11);
        assert_eq!(names, vec!["main".to_owned()]);
    }

    #[test]
    fn kv_and_database_verbs_are_planned_with_their_arguments() {
        let resources = Resources {
            namespaces: vec!["users".to_owned()],
            databases: vec!["main".to_owned()],
        };
        let Plan::Kv {
            namespace,
            op,
            key,
            value,
        } = plan(
            r#"kv("users"):set("ada", { admin = true })"#,
            &classes(),
            &resources,
        )
        .expect("plans")
        else {
            panic!("a kv plan");
        };
        assert_eq!((namespace.as_str(), op.as_str()), ("users", "set"));
        assert_eq!(key.as_deref(), Some("ada"));
        assert_eq!(value, Some(json!({ "admin": true })));
        let Plan::Sql {
            database,
            op,
            sql,
            params,
        } = plan(
            r#"database "main":query("select * from lots where owner = ?", { "ada" })"#,
            &classes(),
            &resources,
        )
        .expect("plans")
        else {
            panic!("a sql plan");
        };
        assert_eq!((database.as_str(), op.as_str()), ("main", "query"));
        assert!(sql.starts_with("select"));
        assert_eq!(params, vec![json!("ada")]);
        let error = plan(r#"kv("nope"):get("k")"#, &classes(), &resources).expect_err("refused");
        assert!(error.contains("not in this project"), "{error}");
    }
}
