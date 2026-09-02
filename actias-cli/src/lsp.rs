//! `actias lsp`: the language service, spoken to editors.
//!
//! This is a transport, not a second analyser. Every answer comes from
//! the same `actias-luau` the workbench loads as wasm and `actias check`
//! runs, so a squiggle in VS Code, a squiggle in the browser editor and
//! a failure in CI are the same opinion. That is the whole reason to
//! write this rather than point editors at a general Luau server: a
//! general one knows nothing about the platform's globals or about
//! bundle-relative requires, and would disagree with the two surfaces
//! you already have.
//!
//! Editors talk JSON-RPC over stdio, framed with `Content-Length`
//! headers. Positions are zero-based there and one-based over the
//! shadowed text the service sees; [`crate::service::Shadow`] is the
//! only place that arithmetic lives.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::service::{Service, Shadow};

/// Runs the server until the editor closes stdin or says `exit`.
///
/// # Errors
/// Returns text when the service is missing or the stream breaks.
pub fn serve() -> Result<(), String> {
    let command = crate::service::locate().ok_or(
        "actias-luau is not installed; the language server needs it. \
         Build it with luau-web/build.sh --native, or put it on PATH.",
    )?;

    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let stdout = std::io::stdout();
    let mut output = stdout.lock();

    let mut session = Session::new(Service::start(&command)?);

    while let Some(message) = read_message(&mut input)? {
        let id = message.get("id").cloned();
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();

        if method == "exit" {
            break;
        }

        match session.handle(&method, message.get("params")) {
            Outcome::Reply(result) => {
                if let Some(id) = id {
                    write_message(
                        &mut output,
                        &json!({"jsonrpc":"2.0","id":id,"result":result}),
                    )?;
                }
            }
            Outcome::Nothing => {
                // A request still owes a reply even when nothing came of
                // it; a notification owes none.
                if let Some(id) = id {
                    write_message(
                        &mut output,
                        &json!({"jsonrpc":"2.0","id":id,"result":Value::Null}),
                    )?;
                }
            }
            Outcome::Failed(message) => {
                if let Some(id) = id {
                    write_message(
                        &mut output,
                        &json!({"jsonrpc":"2.0","id":id,
                                "error":{"code":-32603,"message":message}}),
                    )?;
                }
            }
        }

        // Diagnostics are pushed, never asked for, so they are published
        // after whatever changed the project.
        for note in session.take_publications() {
            write_message(&mut output, &note)?;
        }
    }

    Ok(())
}

/// One open project: the files the service knows, and how each maps back
/// to what the user wrote.
struct Session {
    service: Service,
    prologue: String,
    /// Bundle path to its shadow map, for every module loaded.
    shadows: HashMap<String, Shadow>,
    /// Bundle path to the uri it was opened as, for publishing back.
    uris: HashMap<String, String>,
    /// The project root, once a file has told us where it is.
    root: Option<PathBuf>,
    pending: Vec<Value>,
}

enum Outcome {
    Reply(Value),
    Nothing,
    Failed(String),
}

impl Session {
    fn new(service: Service) -> Self {
        Self {
            service,
            prologue: crate::analyze::prologue(true),
            shadows: HashMap::new(),
            uris: HashMap::new(),
            root: None,
            pending: Vec::new(),
        }
    }

    fn take_publications(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.pending)
    }

    fn handle(&mut self, method: &str, params: Option<&Value>) -> Outcome {
        let params = params.cloned().unwrap_or(Value::Null);
        match method {
            "initialize" => Outcome::Reply(Self::capabilities()),
            "initialized" | "shutdown" => Outcome::Nothing,

            "textDocument/didOpen" => self.opened(&params),
            "textDocument/didChange" => self.changed(&params),
            "textDocument/didClose" => self.closed(&params),
            // A save changes nothing the editor has not already sent.
            "textDocument/didSave" => Outcome::Nothing,

            "textDocument/hover" => self.hover(&params),
            "textDocument/completion" => self.completion(&params),
            "textDocument/definition" => self.definition(&params),
            "textDocument/signatureHelp" => self.signature(&params),

            _ => Outcome::Nothing,
        }
    }

    /// What this server can do, in the shape `initialize` expects. Full
    /// text sync because the service replaces whole files anyway, so
    /// incremental sync would only add a patching step to get back to
    /// the same string.
    fn capabilities() -> Value {
        json!({
            "capabilities": {
                "textDocumentSync": 1,
                "hoverProvider": true,
                "definitionProvider": true,
                "completionProvider": { "triggerCharacters": [".", ":"] },
                "signatureHelpProvider": { "triggerCharacters": ["(", ","] }
            },
            "serverInfo": { "name": "actias", "version": env!("CARGO_PKG_VERSION") }
        })
    }

    /// Loads every Lua file of the project the opened file belongs to.
    /// The whole bundle, not just this file: a `require` only resolves
    /// against modules the service has been given.
    fn opened(&mut self, params: &Value) -> Outcome {
        let Some(uri) = params.pointer("/textDocument/uri").and_then(Value::as_str) else {
            return Outcome::Nothing;
        };
        let Some(path) = uri_to_path(uri) else {
            return Outcome::Nothing;
        };

        if self.root.is_none()
            && let Some(found) = project_root(&path)
        {
            self.root = Some(found.clone());
            if let Err(error) = self.load_project(&found) {
                return Outcome::Failed(error);
            }
        }

        // The editor's copy wins over what is on disk, since it may hold
        // edits that were never saved.
        if let Some(text) = params.pointer("/textDocument/text").and_then(Value::as_str)
            && let Some(module) = self.module_of(&path)
        {
            self.uris.insert(module.clone(), uri.to_owned());
            if let Err(error) = self.load(&module, text) {
                return Outcome::Failed(error);
            }
        }

        self.publish_all();
        Outcome::Nothing
    }

    fn changed(&mut self, params: &Value) -> Outcome {
        let Some(uri) = params.pointer("/textDocument/uri").and_then(Value::as_str) else {
            return Outcome::Nothing;
        };
        // Full sync, so the last change carries the whole document.
        let Some(text) = params
            .pointer("/contentChanges")
            .and_then(Value::as_array)
            .and_then(|changes| changes.last())
            .and_then(|change| change.get("text"))
            .and_then(Value::as_str)
        else {
            return Outcome::Nothing;
        };
        let Some(module) = uri_to_path(uri).and_then(|path| self.module_of(&path)) else {
            return Outcome::Nothing;
        };

        if let Err(error) = self.load(&module, text) {
            return Outcome::Failed(error);
        }
        self.publish_all();
        Outcome::Nothing
    }

    fn closed(&mut self, params: &Value) -> Outcome {
        // The file stays loaded: it is still part of the project, and
        // other modules requiring it must keep type-checking.
        let _ = params;
        Outcome::Nothing
    }

    fn hover(&mut self, params: &Value) -> Outcome {
        self.positioned("hover", params, |answer, _shadow| {
            let text = answer.get("type").and_then(Value::as_str)?;
            Some(json!({
                "contents": { "kind": "markdown", "value": format!("```luau\n{text}\n```") }
            }))
        })
    }

    fn completion(&mut self, params: &Value) -> Outcome {
        self.positioned("complete", params, |answer, _shadow| {
            let entries = answer.as_array()?;
            let items: Vec<Value> = entries
                .iter()
                .filter_map(|entry| {
                    let label = entry.get("label").and_then(Value::as_str)?;
                    Some(json!({
                        "label": label,
                        "kind": completion_kind(entry.get("kind").and_then(Value::as_str)),
                        "detail": entry.get("detail").and_then(Value::as_str),
                    }))
                })
                .collect();
            Some(json!({ "isIncomplete": false, "items": items }))
        })
    }

    /// Go to definition, which lands in whichever module defines the
    /// symbol and so cannot use the asking file's map: files differ in
    /// how many `--!` directives sit above their prologue, so the target
    /// has to be measured by its own [`Shadow`].
    fn definition(&mut self, params: &Value) -> Outcome {
        let answer = match self.ask("definition", params) {
            Ok(Some(answer)) => answer,
            Ok(None) => return Outcome::Nothing,
            Err(error) => return Outcome::Failed(error),
        };

        let (Some(module), Some(line)) = (
            answer.get("module").and_then(Value::as_str),
            answer.get("line").and_then(Value::as_u64),
        ) else {
            return Outcome::Nothing;
        };
        let column = answer.get("column").and_then(Value::as_u64).unwrap_or(1) as usize;

        let Some(shadow) = self.shadows.get(module) else {
            return Outcome::Nothing;
        };
        let Some(user) = shadow.to_user(line as usize) else {
            return Outcome::Nothing;
        };

        // A definition may land in a module the editor never opened, so
        // its uri is built from the root rather than looked up.
        let uri = match self.uris.get(module) {
            Some(known) => known.clone(),
            None => match &self.root {
                Some(root) => format!("file://{}", root.join(module).display()),
                None => return Outcome::Nothing,
            },
        };

        let character = column.saturating_sub(1);
        Outcome::Reply(json!({
            "uri": uri,
            "range": {
                "start": { "line": user, "character": character },
                "end":   { "line": user, "character": character }
            }
        }))
    }

    fn signature(&mut self, params: &Value) -> Outcome {
        self.positioned("signature", params, |answer, _shadow| {
            let label = answer.get("signature").and_then(Value::as_str)?;
            let active = answer.get("active").and_then(Value::as_u64).unwrap_or(0);
            Some(json!({
                "signatures": [{ "label": label }],
                "activeSignature": 0,
                "activeParameter": active
            }))
        })
    }

    /// Puts one positioned question to the service, translating the
    /// editor's position on the way in. [`None`] means there was nothing
    /// to ask about or nothing to say.
    ///
    /// # Errors
    /// Returns text when the service cannot answer.
    fn ask(&mut self, op: &str, params: &Value) -> Result<Option<Value>, String> {
        let Some(uri) = params.pointer("/textDocument/uri").and_then(Value::as_str) else {
            return Ok(None);
        };
        let Some(module) = uri_to_path(uri).and_then(|path| self.module_of(&path)) else {
            return Ok(None);
        };
        let (Some(line), Some(character)) = (
            params.pointer("/position/line").and_then(Value::as_u64),
            params
                .pointer("/position/character")
                .and_then(Value::as_u64),
        ) else {
            return Ok(None);
        };
        let Some(shadow) = self.shadows.get(&module) else {
            return Ok(None);
        };

        let service_line = shadow.to_service(line as usize);
        let answer = self
            .service
            .at(op, &module, service_line, character as usize + 1)?;
        Ok((!answer.is_null()).then_some(answer))
    }

    /// The shared shape of the requests whose answer stays inside the
    /// file that asked: hover, completion, signature help.
    fn positioned<F>(&mut self, op: &str, params: &Value, shape: F) -> Outcome
    where
        F: FnOnce(&Value, &Shadow) -> Option<Value>,
    {
        let module = match params
            .pointer("/textDocument/uri")
            .and_then(Value::as_str)
            .and_then(uri_to_path)
            .and_then(|path| self.module_of(&path))
        {
            Some(module) => module,
            None => return Outcome::Nothing,
        };

        let answer = match self.ask(op, params) {
            Ok(Some(answer)) => answer,
            Ok(None) => return Outcome::Nothing,
            Err(error) => return Outcome::Failed(error),
        };

        let Some(shadow) = self.shadows.get(&module) else {
            return Outcome::Nothing;
        };
        match shape(&answer, shadow) {
            Some(value) => Outcome::Reply(value),
            None => Outcome::Nothing,
        }
    }

    /// Reads the project's bundle off disk and hands every Lua module to
    /// the service.
    fn load_project(&mut self, root: &Path) -> Result<(), String> {
        let config = crate::script::ScriptConfig::from_path(root)?;
        let bundle = config.to_bundle()?;

        for file in &bundle.files {
            if !file.file_path.ends_with(".lua") {
                continue;
            }
            let bytes = base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD_NO_PAD,
                &file.content,
            )
            .map_err(|error| format!("{}: {error}", file.file_path))?;
            let Ok(source) = String::from_utf8(bytes) else {
                continue;
            };
            self.load(&file.file_path.clone(), &source)?;
        }
        Ok(())
    }

    /// One module into the service, with its map recorded.
    fn load(&mut self, module: &str, source: &str) -> Result<(), String> {
        self.shadows
            .insert(module.to_owned(), Shadow::new(source, &self.prologue));
        let shadowed = crate::analyze::shadow_source(source, &self.prologue);
        self.service.set_file(module, &shadowed)
    }

    /// Re-checks every module the editor has a uri for. Cheap enough:
    /// the frontend only re-checks what an edit actually dirtied.
    fn publish_all(&mut self) {
        let modules: Vec<String> = self.uris.keys().cloned().collect();
        for module in modules {
            let Ok(found) = self.service.check(&module) else {
                continue;
            };
            let Some(shadow) = self.shadows.get(&module) else {
                continue;
            };

            let diagnostics: Vec<Value> = found
                .iter()
                .filter_map(|diagnostic| {
                    let start = shadow.to_user(diagnostic.line)?;
                    let end = shadow.to_user(diagnostic.end_line).unwrap_or(start);
                    Some(json!({
                        "range": {
                            "start": { "line": start,
                                       "character": diagnostic.column.saturating_sub(1) },
                            "end":   { "line": end,
                                       "character": diagnostic.end_column.saturating_sub(1) }
                        },
                        // 1 error, 2 warning; a lint is advice, not a failure.
                        "severity": if diagnostic.severity == "error" { 1 } else { 2 },
                        "source": "actias",
                        "message": diagnostic.message,
                    }))
                })
                .collect();

            if let Some(uri) = self.uris.get(&module) {
                self.pending.push(json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/publishDiagnostics",
                    "params": { "uri": uri, "diagnostics": diagnostics }
                }));
            }
        }
    }

    /// A file's bundle-relative path, which is the name the service and
    /// every `require` know it by.
    fn module_of(&self, path: &Path) -> Option<String> {
        let root = self.root.as_ref()?;
        let relative = path.strip_prefix(root).ok()?;
        Some(relative.to_string_lossy().replace('\\', "/"))
    }
}

/// Walks up for the `script.json` that marks a project root.
fn project_root(from: &Path) -> Option<PathBuf> {
    let mut at = from.parent();
    while let Some(directory) = at {
        if directory.join("script.json").exists() {
            return Some(directory.to_path_buf());
        }
        at = directory.parent();
    }
    None
}

/// `file:///a/b.lua` to a path. Only the file scheme means anything
/// here; a project lives on disk.
fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);

    let mut decoded = String::with_capacity(rest.len());
    let bytes = rest.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(&rest[index + 1..index + 3], 16)
        {
            decoded.push(byte as char);
            index += 3;
            continue;
        }
        decoded.push(bytes[index] as char);
        index += 1;
    }
    Some(PathBuf::from(decoded))
}

/// The service's completion kinds in the numbers LSP uses for them.
fn completion_kind(kind: Option<&str>) -> u8 {
    match kind {
        Some("property") => 5,
        Some("binding") => 6,
        Some("keyword") => 14,
        Some("string") => 15,
        Some("type") => 22,
        Some("module") => 9,
        _ => 1,
    }
}

/// Reads one `Content-Length` framed message, or [`None`] at end of
/// stream.
///
/// # Errors
/// Returns text when the stream breaks or the framing is malformed.
fn read_message(input: &mut impl BufRead) -> Result<Option<Value>, String> {
    let mut length = None;
    loop {
        let mut header = String::new();
        if input.read_line(&mut header).map_err(stream)? == 0 {
            return Ok(None);
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some(value) = header.strip_prefix("Content-Length:") {
            length = value.trim().parse::<usize>().ok();
        }
    }

    let length = length.ok_or("a message arrived without a Content-Length")?;
    let mut body = vec![0u8; length];
    input.read_exact(&mut body).map_err(stream)?;
    serde_json::from_slice(&body).map(Some).map_err(stream)
}

/// # Errors
/// Returns text when the stream breaks.
fn write_message(output: &mut impl Write, message: &Value) -> Result<(), String> {
    let body = serde_json::to_vec(message).map_err(stream)?;
    write!(output, "Content-Length: {}\r\n\r\n", body.len()).map_err(stream)?;
    output.write_all(&body).map_err(stream)?;
    output.flush().map_err(stream)
}

fn stream(error: impl std::fmt::Display) -> String {
    format!("the editor connection broke: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_become_paths() {
        assert_eq!(
            uri_to_path("file:///home/a/main.lua"),
            Some(PathBuf::from("/home/a/main.lua"))
        );
        // Editors percent-encode spaces, and a project may well sit in
        // a directory that has one.
        assert_eq!(
            uri_to_path("file:///home/my%20project/main.lua"),
            Some(PathBuf::from("/home/my project/main.lua"))
        );
        assert_eq!(uri_to_path("untitled:one"), None);
    }

    #[test]
    fn framing_round_trips() {
        let mut written = Vec::new();
        write_message(&mut written, &json!({"jsonrpc":"2.0","id":1})).expect("writes");
        let text = String::from_utf8(written.clone()).expect("utf8");
        assert!(text.starts_with("Content-Length: "));

        let mut reader = std::io::BufReader::new(written.as_slice());
        let read = read_message(&mut reader)
            .expect("reads")
            .expect("a message");
        assert_eq!(read["id"], 1);
    }
}
