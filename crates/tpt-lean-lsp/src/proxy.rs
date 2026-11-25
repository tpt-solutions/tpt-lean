// SPDX-License-Identifier: MIT OR Apache-2.0
//! The proxy loop. Editor messages come in on stdin; some requests are answered locally
//! (initialize, document symbols, workspace symbols), the rest are forwarded to Lean through an
//! [`LspSession`]. Lean's diagnostics are forwarded when they change, and the last set is cached
//! so that reopening an unchanged file shows diagnostics before Lean has elaborated it.

use crate::framing::{read_message, write_message};
use crate::symbols::document_symbols;
use crate::workspace::{definitions, import_modules, workspace_symbols};
use serde_json::{json, Value};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tpt_lean_adapter::explain::{error_at, lemma_candidates, split_goal};
use tpt_lean_adapter::lsp::{is_cancelled, parse_plain_goals, path_to_uri};
use tpt_lean_adapter::LspSession;
use tpt_lean_core::dirs::ProjectDirs;
use tpt_lean_core::policy::CancelToken;
use tpt_lean_core::{Error, ErrorCode, Result};

/// How often the loop checks Lean for new diagnostics when no editor message arrives.
const POLL: Duration = Duration::from_millis(50);
/// Timeout for one request forwarded to Lean.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const WORKSPACE_SYMBOL_LIMIT: usize = 200;

const METHOD_NOT_FOUND: i64 = -32601;
const INTERNAL_ERROR: i64 = -32603;
const REQUEST_CANCELLED: i64 = -32800;
const LOG_ERROR: u8 = 1;

/// Shared table of in-flight editor requests and their cancel tokens, keyed by request id.
type CancelTable = Arc<Mutex<HashMap<String, CancelToken>>>;

#[derive(PartialEq, Eq)]
enum Flow {
    Continue,
    Exit,
}

/// A document as the editor has it.
struct Document {
    /// URI the editor uses; echoed back in notifications.
    editor_uri: String,
    /// URI the Lean server uses (derived from the file path).
    upstream_uri: String,
    text_hash: u64,
    symbols: Vec<Value>,
    /// Full text, kept for answers that need the word under the cursor.
    text: String,
}

/// Diagnostics last computed for a document, kept after the document is closed.
struct CachedDiagnostics {
    text_hash: u64,
    diagnostics: Vec<Value>,
}

pub struct Proxy<W: Write> {
    index_path: PathBuf,
    project_root: PathBuf,
    session: LspSession,
    out: W,
    /// Open documents keyed by the editor's URI.
    documents: HashMap<String, Document>,
    /// Diagnostics last sent to the editor, keyed by the editor's URI.
    published: HashMap<String, Vec<Value>>,
    cache: HashMap<String, CachedDiagnostics>,
    cancels: CancelTable,
}

impl<W: Write> Proxy<W> {
    /// A proxy for the project at `project_root`, forwarding to `session`, writing to `out`.
    pub fn new(project_root: &Path, session: LspSession, out: W) -> Self {
        Self {
            index_path: ProjectDirs::new(project_root).index_db(),
            project_root: project_root.to_path_buf(),
            session,
            out,
            documents: HashMap::new(),
            published: HashMap::new(),
            cache: HashMap::new(),
            cancels: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Everything written to the editor so far (useful for tests).
    pub fn output(&self) -> &W {
        &self.out
    }

    /// Serve editor messages from `input` until `exit` or end of input. Stops the Lean server.
    pub fn serve<R: BufRead + Send + 'static>(&mut self, input: R) -> Result<()> {
        let (tx, rx) = channel();
        let cancels = Arc::clone(&self.cancels);
        thread::spawn(move || reader_loop(input, tx, cancels));
        loop {
            match rx.recv_timeout(POLL) {
                Ok(msg) => {
                    if self.handle(msg)? == Flow::Exit {
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            self.forward_new_diagnostics()?;
        }
        self.session.stop();
        Ok(())
    }

    fn handle(&mut self, msg: Value) -> Result<Flow> {
        let method = msg.get("method").and_then(Value::as_str).map(str::to_string);
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        match (method, msg.get("id").cloned()) {
            (Some(method), Some(id)) => self.handle_request(&method, id, &params).map(|()| Flow::Continue),
            (Some(method), None) => self.handle_notification(&method, &params),
            // Responses to requests the proxy sent (none today) are ignored.
            (None, _) => Ok(Flow::Continue),
        }
    }

    fn handle_request(&mut self, method: &str, id: Value, params: &Value) -> Result<()> {
        let key = id.to_string();
        let cancel = self
            .cancels
            .lock()
            .ok()
            .and_then(|t| t.get(&key).cloned())
            .unwrap_or_default();
        let outcome = self.dispatch(method, params, &cancel);
        if let Ok(mut table) = self.cancels.lock() {
            table.remove(&key);
        }
        match outcome {
            Ok(result) => self.send(json!({ "jsonrpc": "2.0", "id": id, "result": result })),
            Err(e) if is_cancelled(&e) => self.send_error(id, REQUEST_CANCELLED, "request cancelled"),
            Err(e) => {
                let code = if e.code() == ErrorCode::NotImplemented {
                    METHOD_NOT_FOUND
                } else {
                    INTERNAL_ERROR
                };
                self.send_error(id, code, e.message())
            }
        }
    }

    fn dispatch(&mut self, method: &str, params: &Value, cancel: &CancelToken) -> Result<Value> {
        match method {
            "initialize" => Ok(initialize_result()),
            "shutdown" => Ok(Value::Null),
            "textDocument/documentSymbol" => Ok(self.document_symbols(params)),
            "workspace/symbol" => {
                let query = params["query"].as_str().unwrap_or("");
                let symbols = workspace_symbols(
                    &self.project_root,
                    &self.index_path,
                    query,
                    WORKSPACE_SYMBOL_LIMIT,
                )?;
                Ok(Value::Array(symbols))
            }
            "textDocument/hover" => self.hover(params, cancel),
            "textDocument/definition" => self.definition(params, cancel),
            "textDocument/codeAction" => Ok(self.code_actions(params, cancel)),
            "workspace/executeCommand" => self.execute_command(params, cancel),
            _ => {
                let upstream = self.upstream_params(params);
                self.session
                    .request_cancellable(method, upstream, cancel)
                    .map_err(|e| with_context(e, method))
            }
        }
    }

    fn handle_notification(&mut self, method: &str, params: &Value) -> Result<Flow> {
        match method {
            "exit" => return Ok(Flow::Exit),
            "textDocument/didOpen" => {
                let uri = params["textDocument"]["uri"].as_str().unwrap_or_default();
                let text = params["textDocument"]["text"].as_str().unwrap_or_default();
                self.store_document(uri, text)?;
            }
            "textDocument/didChange" => {
                let uri = params["textDocument"]["uri"].as_str().unwrap_or_default();
                // Full sync is advertised, so the last change carries the whole text.
                let text = params["contentChanges"]
                    .as_array()
                    .and_then(|c| c.last())
                    .and_then(|c| c["text"].as_str());
                if let Some(text) = text {
                    self.store_document(uri, text)?;
                }
            }
            "textDocument/didClose" => {
                let uri = params["textDocument"]["uri"].as_str().unwrap_or_default();
                self.close_document(uri)?;
            }
            _ => {}
        }
        Ok(Flow::Continue)
    }

    /// Record the text, refresh the cached symbols, sync Lean, and re-publish cached
    /// diagnostics when the text is the same as when they were computed.
    fn store_document(&mut self, editor_uri: &str, text: &str) -> Result<()> {
        let Some(path) = uri_to_path(editor_uri) else {
            return self.log(&format!("unsupported document URI: {editor_uri}"));
        };
        let text_hash = hash_text(text);
        let upstream_uri = path_to_uri(&path);
        self.documents.insert(
            editor_uri.to_string(),
            Document {
                editor_uri: editor_uri.to_string(),
                upstream_uri,
                text_hash,
                symbols: document_symbols(text),
                text: text.to_string(),
            },
        );
        if let Err(e) = self.session.sync_document(&path, text) {
            self.log(&format!("cannot sync {editor_uri} with Lean: {}", e.message()))?;
        }
        let cached = self
            .cache
            .get(editor_uri)
            .filter(|c| c.text_hash == text_hash)
            .map(|c| c.diagnostics.clone());
        if let Some(diagnostics) = cached {
            self.publish(editor_uri, diagnostics)?;
        }
        Ok(())
    }

    fn close_document(&mut self, editor_uri: &str) -> Result<()> {
        if let Some(doc) = self.documents.remove(editor_uri) {
            if let Some(path) = uri_to_path(&doc.editor_uri) {
                let _ = self.session.close_document(&path);
            }
        }
        self.published.remove(editor_uri);
        Ok(())
    }

    /// Send Lean's diagnostics to the editor for every open document whose set has changed.
    fn forward_new_diagnostics(&mut self) -> Result<()> {
        let open: Vec<(String, String)> = self
            .documents
            .values()
            .map(|d| (d.editor_uri.clone(), d.upstream_uri.clone()))
            .collect();
        for (editor_uri, upstream_uri) in open {
            let Some(current) = self.session.published_diagnostics(&upstream_uri) else {
                continue;
            };
            let previous = self.published.get(&editor_uri).cloned().unwrap_or_default();
            if current != previous {
                self.publish(&editor_uri, current)?;
            }
        }
        Ok(())
    }

    fn publish(&mut self, editor_uri: &str, diagnostics: Vec<Value>) -> Result<()> {
        self.send(json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": { "uri": editor_uri, "diagnostics": diagnostics }
        }))?;
        if let Some(doc) = self.documents.get(editor_uri) {
            self.cache.insert(
                editor_uri.to_string(),
                CachedDiagnostics {
                    text_hash: doc.text_hash,
                    diagnostics: diagnostics.clone(),
                },
            );
        }
        self.published.insert(editor_uri.to_string(), diagnostics);
        Ok(())
    }

    fn document_symbols(&self, params: &Value) -> Value {
        let uri = params["textDocument"]["uri"].as_str().unwrap_or_default();
        match self.documents.get(uri) {
            Some(doc) => Value::Array(doc.symbols.clone()),
            None => Value::Array(Vec::new()),
        }
    }

    /// Go-to-definition: when the index has declarations whose exact name is the word under the
    /// cursor, answer from the index. Otherwise forward to Lean.
    fn definition(&mut self, params: &Value, cancel: &CancelToken) -> Result<Value> {
        let uri = params["textDocument"]["uri"].as_str().unwrap_or_default();
        let line = params["position"]["line"].as_u64().unwrap_or(0) as usize;
        let character = params["position"]["character"].as_u64().unwrap_or(0) as usize;
        let word = self
            .documents
            .get(uri)
            .and_then(|doc| word_at(&doc.text, line, character));
        if let Some(word) = word {
            let locations = definitions(&self.project_root, &self.index_path, &word, 20)?;
            if !locations.is_empty() {
                return Ok(Value::Array(locations));
            }
        }
        let upstream = self.upstream_params(params);
        self.session
            .request_cancellable("textDocument/definition", upstream, cancel)
            .map_err(|e| with_context(e, "textDocument/definition"))
    }

    /// Quick fixes (an import for an unknown identifier) and the commands for the lemma search and
    /// the error explanation. Lean's own code actions are forwarded and appended; if Lean does not
    /// answer, only the local actions are returned.
    fn code_actions(&mut self, params: &Value, cancel: &CancelToken) -> Value {
        let uri = params["textDocument"]["uri"].as_str().unwrap_or_default().to_string();
        let range = params["range"].clone();
        let diagnostics: Vec<Value> = params["context"]["diagnostics"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut actions: Vec<Value> = Vec::new();
        let Some(doc) = self.documents.get(&uri) else {
            return Value::Array(actions);
        };
        let text = doc.text.clone();
        let line = import_insert_line(&text);
        for d in &diagnostics {
            let Some(name) = d["message"].as_str().and_then(unknown_name) else {
                continue;
            };
            let modules = import_modules(&self.project_root, &self.index_path, &name, 3).unwrap_or_default();
            for module in modules {
                if already_imports(&text, &module) {
                    continue;
                }
                let at = json!({ "line": line, "character": 0 });
                actions.push(json!({
                    "title": format!("Import {module}"),
                    "kind": "quickfix",
                    "diagnostics": [d],
                    "edit": { "changes": { uri.clone(): [
                        { "range": { "start": at, "end": at }, "newText": format!("import {module}\n") }
                    ] } }
                }));
            }
        }
        let position = json!({ "uri": uri, "line": range["start"]["line"], "character": range["start"]["character"] });
        actions.push(json!({
            "title": "Search lemmas for this goal",
            "kind": "source",
            "command": { "title": "Search lemmas for this goal", "command": SEARCH_LEMMAS, "arguments": [position.clone()] }
        }));
        if !diagnostics.is_empty() {
            actions.push(json!({
                "title": "Explain this error",
                "kind": "source",
                "command": { "title": "Explain this error", "command": EXPLAIN_ERROR, "arguments": [position] }
            }));
        }
        if let Ok(upstream) = self.session.request_cancellable(
            "textDocument/codeAction",
            self.upstream_params(params),
            cancel,
        ) {
            if let Some(lean_actions) = upstream.as_array() {
                actions.extend(lean_actions.iter().cloned());
            }
        }
        Value::Array(actions)
    }

    /// Run one of this server's commands. The argument is `{uri, line, character}`.
    fn execute_command(&mut self, params: &Value, cancel: &CancelToken) -> Result<Value> {
        let command = params["command"].as_str().unwrap_or_default().to_string();
        let arg = params["arguments"][0].clone();
        let uri = arg["uri"].as_str().unwrap_or_default().to_string();
        let line = arg["line"].as_u64().unwrap_or(0);
        let character = arg["character"].as_u64().unwrap_or(0);
        let position_params = json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character }
        });
        match command.as_str() {
            SEARCH_LEMMAS => {
                let goals = self.goals_at(&position_params, cancel)?;
                let target = goals.iter().find_map(|g| split_goal(g).1).unwrap_or_default();
                let lemmas = lemma_candidates(&self.project_root, &target, 10);
                self.log(&format!("tpt-lean: {} lemma candidate(s) for the goal", lemmas.len()))?;
                Ok(Value::Array(lemmas))
            }
            EXPLAIN_ERROR => {
                let diagnostics = self.published.get(&uri).cloned().unwrap_or_default();
                let file = uri_to_path(&uri).map(|p| p.display().to_string()).unwrap_or_default();
                let error = error_at(&diagnostics, &file, line, character);
                // The explanation is still useful without goals (for example when Lean is down).
                let goals = self.goals_at(&position_params, cancel).unwrap_or_default();
                Ok(json!({
                    "error": error.map(|e| json!({ "message": e.message, "line": e.line, "column": e.column })),
                    "goals": goals,
                }))
            }
            other => Err(Error::new(
                ErrorCode::InvalidArgument,
                format!("unknown command {other:?}"),
            )),
        }
    }

    /// Plain goals at a position, from Lean.
    fn goals_at(&mut self, position_params: &Value, cancel: &CancelToken) -> Result<Vec<String>> {
        let upstream = self.upstream_params(position_params);
        let result = self
            .session
            .request_cancellable("$/lean/plainGoal", upstream, cancel)
            .map_err(|e| with_context(e, "$/lean/plainGoal"))?;
        Ok(parse_plain_goals(&result))
    }

    /// Hover shows the goal at the position, from `$/lean/plainGoal`.
    fn hover(&mut self, params: &Value, cancel: &CancelToken) -> Result<Value> {
        let upstream = self.upstream_params(params);
        let result = self
            .session
            .request_cancellable("$/lean/plainGoal", upstream, cancel)
            .map_err(|e| with_context(e, "textDocument/hover"))?;
        let goals = parse_plain_goals(&result);
        if goals.is_empty() {
            return Ok(Value::Null);
        }
        Ok(json!({
            "contents": {
                "kind": "markdown",
                "value": format!("```lean\n{}\n```", goals.join("\n\n"))
            }
        }))
    }

    /// Replace the editor's document URI with the one Lean knows the document by.
    fn upstream_params(&self, params: &Value) -> Value {
        let mut params = params.clone();
        let editor_uri = params["textDocument"]["uri"].as_str().map(str::to_string);
        if let Some(doc) = editor_uri.and_then(|u| self.documents.get(&u)) {
            params["textDocument"]["uri"] = Value::String(doc.upstream_uri.clone());
        }
        params
    }

    fn log(&mut self, message: &str) -> Result<()> {
        self.send(json!({
            "jsonrpc": "2.0",
            "method": "window/logMessage",
            "params": { "type": LOG_ERROR, "message": message }
        }))
    }

    fn send_error(&mut self, id: Value, code: i64, message: &str) -> Result<()> {
        self.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message }
        }))
    }

    fn send(&mut self, message: Value) -> Result<()> {
        write_message(&mut self.out, &message)
    }
}

/// Program entry point: serve the editor on stdin/stdout for the project at `project_root`.
/// Lean is started on first use with `lake serve` or `lean --server`.
pub fn run_stdio(project_root: &Path) -> Result<()> {
    let toolchain = tpt_lean_adapter::discover_toolchain();
    let session = LspSession::for_project(
        project_root,
        toolchain.lake.as_ref(),
        toolchain.lean.as_ref(),
        REQUEST_TIMEOUT,
    )?;
    let stdout = std::io::stdout();
    let mut proxy = Proxy::new(project_root, session, stdout.lock());
    proxy.serve(std::io::BufReader::new(std::io::stdin()))
}

/// Read editor messages and handle cancellation right away. Cancellation must not wait for the
/// main loop, which may be blocked inside a forwarded request.
fn reader_loop<R: BufRead>(mut input: R, tx: Sender<Value>, cancels: CancelTable) {
    loop {
        let msg = match read_message(&mut input) {
            Ok(Some(m)) => m,
            Ok(None) | Err(_) => return,
        };
        let method = msg.get("method").and_then(Value::as_str).unwrap_or_default();
        if method == "$/cancelRequest" {
            let key = msg["params"]["id"].to_string();
            if let Ok(table) = cancels.lock() {
                if let Some(token) = table.get(&key) {
                    token.cancel();
                }
            }
            continue;
        }
        if msg.get("id").is_some() && !method.is_empty() {
            if let Ok(mut table) = cancels.lock() {
                table.insert(msg["id"].to_string(), CancelToken::new());
            }
        }
        if tx.send(msg).is_err() {
            return;
        }
    }
}

/// Command that searches the index for lemmas about the goal at a position.
pub const SEARCH_LEMMAS: &str = "tpt-lean.searchLemmas";
/// Command that explains the error at a position, with the goal there.
pub const EXPLAIN_ERROR: &str = "tpt-lean.explainError";

/// The name in a Lean "unknown identifier 'x'" or "unknown constant 'x'" message.
pub fn unknown_name(message: &str) -> Option<String> {
    for prefix in ["unknown identifier '", "unknown constant '"] {
        if let Some(rest) = message.split(prefix).nth(1) {
            let name = rest.split('\'').next()?;
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

/// Line where a new `import` can go: after the last existing import, or the first line.
pub fn import_insert_line(text: &str) -> u64 {
    text.lines()
        .enumerate()
        .filter(|(_, l)| l.trim_start().starts_with("import "))
        .map(|(i, _)| i as u64 + 1)
        .last()
        .unwrap_or(0)
}

/// Whether the document already imports `module`.
pub fn already_imports(text: &str, module: &str) -> bool {
    text.lines().any(|l| l.trim() == format!("import {module}"))
}

fn initialize_result() -> Value {
    json!({
        "capabilities": {
            "textDocumentSync": { "openClose": true, "change": 1 },
            "documentSymbolProvider": true,
            "workspaceSymbolProvider": true,
            "hoverProvider": true,
            "codeActionProvider": { "codeActionKinds": ["quickfix", "source"] },
            "executeCommandProvider": { "commands": [SEARCH_LEMMAS, EXPLAIN_ERROR] }
        },
        "serverInfo": { "name": "tpt-lean-lsp", "version": env!("CARGO_PKG_VERSION") }
    })
}

fn with_context(e: Error, method: &str) -> Error {
    if is_cancelled(&e) {
        return e;
    }
    Error::new(e.code(), format!("{method}: {}", e.message()))
}

fn hash_text(text: &str) -> u64 {
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// Local path of a `file://` URI. Percent-escapes are decoded and a Windows drive letter is
/// kept without the leading slash. Returns `None` for other schemes.
/// Characters that can appear in a Lean identifier or a dotted name.
fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '\'' | '.' | '!' | '?')
}

/// The dotted identifier under an LSP position (`line` and UTF-16 `character`, both 0-based).
/// A position just after an identifier also selects it.
pub fn word_at(text: &str, line: usize, character: usize) -> Option<String> {
    let line_text = text.lines().nth(line)?;
    let chars: Vec<char> = line_text.chars().collect();
    // Map the UTF-16 column to a char index.
    let mut utf16 = 0;
    let mut index = chars.len();
    for (i, c) in chars.iter().enumerate() {
        if utf16 >= character {
            index = i;
            break;
        }
        utf16 += c.len_utf16();
    }
    let probe = if index < chars.len() && is_name_char(chars[index]) {
        index
    } else if index > 0 && is_name_char(chars[index - 1]) {
        index - 1
    } else {
        return None;
    };
    let mut start = probe;
    while start > 0 && is_name_char(chars[start - 1]) {
        start -= 1;
    }
    let mut end = probe + 1;
    while end < chars.len() && is_name_char(chars[end]) {
        end += 1;
    }
    let word: String = chars[start..end].iter().collect();
    let word = word.trim_matches('.').to_string();
    (!word.is_empty()).then_some(word)
}

pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut decoded: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    let mut path = String::from_utf8(decoded).ok()?;
    let drive = path.as_bytes();
    if drive.len() >= 3 && drive[0] == b'/' && drive[1].is_ascii_alphabetic() && drive[2] == b':' {
        path.remove(0);
    }
    Some(PathBuf::from(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Cursor};
    use std::time::Duration;

    #[test]
    fn code_actions_offer_imports_and_commands_without_lean() {
        let dir = std::env::temp_dir().join(format!("tpt-lsp-actions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".tpt-lean")).unwrap();
        std::fs::write(dir.join("Lib.lean"), "theorem widget_size_ok : True := trivial
").unwrap();
        let mut index = tpt_lean_index::Index::open(&ProjectDirs::new(&dir).index_db()).unwrap();
        index.refresh(&tpt_lean_index::default_scan_roots(&dir, false)).unwrap();
        drop(index);
        let session = LspSession::new(
            Path::new("/nonexistent/tpt-lean-test-server"),
            &[],
            &dir,
            Duration::from_secs(5),
        );
        let uri = "file:///tmp/tpt-proxy-test/Main.lean";
        let text = "import Std

#check widget_size_ok
";
        let range = json!({"start": {"line": 2, "character": 7}, "end": {"line": 2, "character": 21}});
        let diagnostic = json!({
            "range": range, "severity": 1,
            "message": "unknown identifier 'widget_size_ok'"
        });
        let input = frames(&[
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
            json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
            json!({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {
                "textDocument": {"uri": uri, "languageId": "lean4", "version": 1, "text": text}}}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "textDocument/codeAction", "params": {
                "textDocument": {"uri": uri}, "range": range,
                "context": {"diagnostics": [diagnostic]}}}),
            json!({"jsonrpc": "2.0", "id": 3, "method": "workspace/executeCommand", "params": {
                "command": EXPLAIN_ERROR,
                "arguments": [{"uri": uri, "line": 2, "character": 7}]}}),
            json!({"jsonrpc": "2.0", "id": 4, "method": "shutdown"}),
            json!({"jsonrpc": "2.0", "method": "exit"}),
        ]);
        let mut proxy = Proxy::new(&dir, session, Vec::new());
        proxy.serve(BufReader::new(Cursor::new(input))).unwrap();
        let out = read_back(proxy.output());
        let _ = std::fs::remove_dir_all(&dir);

        let init = out.iter().find(|m| m["id"] == 1).expect("initialize");
        assert!(init["result"]["capabilities"]["codeActionProvider"].is_object());
        assert_eq!(init["result"]["capabilities"]["executeCommandProvider"]["commands"][1], EXPLAIN_ERROR);

        let actions = out.iter().find(|m| m["id"] == 2).expect("codeAction")["result"]
            .as_array()
            .unwrap()
            .clone();
        let import = actions
            .iter()
            .find(|a| a["title"] == "Import Lib")
            .expect("import quick fix");
        assert_eq!(import["kind"], "quickfix");
        let edit = &import["edit"]["changes"][uri][0];
        assert_eq!(edit["range"]["start"]["line"], 1);
        assert_eq!(edit["newText"], "import Lib
");
        let titles: Vec<&str> = actions.iter().filter_map(|a| a["title"].as_str()).collect();
        assert!(titles.contains(&"Search lemmas for this goal"), "{titles:?}");
        assert!(titles.contains(&"Explain this error"), "{titles:?}");

        let explained = out.iter().find(|m| m["id"] == 3).expect("explain");
        assert!(explained["result"]["goals"].is_array(), "{explained}");
    }

    #[test]
    fn unknown_names_and_import_lines() {
        assert_eq!(unknown_name("unknown identifier 'Nat.foo'").as_deref(), Some("Nat.foo"));
        assert_eq!(unknown_name("unknown constant 'bar'").as_deref(), Some("bar"));
        assert_eq!(unknown_name("type mismatch"), None);
        assert_eq!(unknown_name("unknown identifier ''"), None);
        let text = "import Std\nimport Lake.Util\n\ndef x := 1\n";
        assert_eq!(import_insert_line(text), 2);
        assert_eq!(import_insert_line("def x := 1\n"), 0);
        assert!(already_imports(text, "Std"));
        assert!(!already_imports(text, "Geo"));
    }

    #[test]
    fn word_at_finds_dotted_names_and_handles_edges() {
        let text = "theorem t : Nat.add_comm a b = b + a := by
  exact «é».foo
";
        assert_eq!(word_at(text, 0, 12).as_deref(), Some("Nat.add_comm"));
        assert_eq!(word_at(text, 0, 23).as_deref(), Some("Nat.add_comm"));
        assert_eq!(word_at(text, 0, 24).as_deref(), Some("Nat.add_comm"));
        assert_eq!(word_at(text, 0, 2).as_deref(), Some("theorem"));
        assert_eq!(word_at(text, 0, 11).as_deref(), None);
        assert_eq!(word_at(text, 5, 0), None);
        assert_eq!(word_at("x := 1", 0, 0).as_deref(), Some("x"));
        assert_eq!(word_at("a ∀ b", 0, 4).as_deref(), Some("b"));
        assert_eq!(word_at("", 0, 0), None);
    }

    #[test]
    fn uri_decoding() {
        assert_eq!(
            uri_to_path("file:///home/a%20b/Main.lean"),
            Some(PathBuf::from("/home/a b/Main.lean"))
        );
        assert_eq!(
            uri_to_path("file:///c%3A/Programming/Main.lean"),
            Some(PathBuf::from("c:/Programming/Main.lean"))
        );
        assert_eq!(uri_to_path("untitled:Untitled-1"), None);
    }

    fn frames(messages: &[Value]) -> Vec<u8> {
        let mut buf = Vec::new();
        for m in messages {
            write_message(&mut buf, m).unwrap();
        }
        buf
    }

    /// Messages the proxy wrote, decoded.
    fn read_back(bytes: &[u8]) -> Vec<Value> {
        let mut reader = BufReader::new(Cursor::new(bytes.to_vec()));
        let mut out = Vec::new();
        while let Some(m) = read_message(&mut reader).unwrap() {
            out.push(m);
        }
        out
    }

    #[test]
    fn answers_locally_without_lean() {
        // The server program does not exist, so every forwarded request would fail.
        let dir = std::env::temp_dir().join(format!("tpt-lsp-proxy-{}", std::process::id()));
        let session = LspSession::new(
            Path::new("/nonexistent/tpt-lean-test-server"),
            &[],
            &dir,
            Duration::from_secs(5),
        );
        let uri = "file:///tmp/tpt-proxy-test/Main.lean";
        let text = "namespace Demo\n\ntheorem alpha : True := trivial\n\nend Demo\n";
        let input = frames(&[
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
            json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
            json!({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {
                "textDocument": {"uri": uri, "languageId": "lean4", "version": 1, "text": text}}}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "textDocument/documentSymbol", "params": {
                "textDocument": {"uri": uri}}}),
            json!({"jsonrpc": "2.0", "id": 3, "method": "shutdown"}),
            json!({"jsonrpc": "2.0", "method": "exit"}),
        ]);
        let mut proxy = Proxy::new(&dir, session, Vec::new());
        proxy.serve(BufReader::new(Cursor::new(input))).unwrap();
        let out = read_back(proxy.output());

        let init = out.iter().find(|m| m["id"] == 1).expect("initialize response");
        assert_eq!(init["result"]["capabilities"]["documentSymbolProvider"], true);
        assert_eq!(init["result"]["serverInfo"]["name"], "tpt-lean-lsp");

        let symbols = out
            .iter()
            .find(|m| m["id"] == 2)
            .expect("documentSymbol response");
        let names: Vec<&str> = symbols["result"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|s| s["name"].as_str())
            .collect();
        assert!(names.contains(&"alpha"), "{names:?}");

        assert!(out.iter().any(|m| m["id"] == 3 && m["result"].is_null()));
        // Lean could not start, so the failure is reported to the editor as a log message.
        assert!(out.iter().any(|m| m["method"] == "window/logMessage"));
    }
}
