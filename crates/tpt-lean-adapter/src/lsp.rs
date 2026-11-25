// SPDX-License-Identifier: MIT OR Apache-2.0
//! Minimal LSP client over stdio (Level 2 of the adapter). It talks to `lake serve` or
//! `lean --server` and asks Lean for goals. Lean stays the source of truth.
//!
//! Framing is the standard `Content-Length` header followed by a JSON body.
//! Server-initiated requests are answered with `null` so the server never blocks on us.
//! Document sync is full-text: every change sends the whole document.
//! [`crate::session::LspSession`] adds restart and crash recovery on top of this client.

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};
use tpt_lean_core::policy::{sanitize_env, CancelToken};
use tpt_lean_core::{Error, ErrorCode, Result};

/// How often a cancellable request re-checks its cancel token.
const CANCEL_POLL: Duration = Duration::from_millis(20);

/// Message of the error returned when a caller cancels a request.
pub const CANCELLED_MESSAGE: &str = "request cancelled";

/// True when `err` was produced by a cancelled request.
pub fn is_cancelled(err: &Error) -> bool {
    err.code() == ErrorCode::Internal && err.message() == CANCELLED_MESSAGE
}

pub struct LspClient {
    child: Child,
    stdin: ChildStdin,
    messages: Receiver<Value>,
    next_id: i64,
    /// Responses that arrived for other request ids while we were waiting.
    pending: Vec<Value>,
    /// Ids of requests the caller gave up on; their late responses are dropped.
    abandoned: HashSet<i64>,
    /// Latest `publishDiagnostics` params per document URI.
    diagnostics: HashMap<String, Value>,
    /// Document versions this client has opened or changed.
    versions: HashMap<String, i64>,
    /// Number of work items Lean still reports as processing, per document (`$/lean/fileProgress`).
    progress: HashMap<String, usize>,
    server_version: Option<String>,
}

/// Convert a path to a `file://` URI, percent-encoding characters that are not URI-safe.
pub fn path_to_uri(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file://");
    if !s.starts_with('/') {
        out.push('/');
    }
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b':' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

impl LspClient {
    /// Start a server process and run the initialize handshake.
    pub fn start(program: &Path, args: &[String], cwd: &Path, timeout: Duration) -> Result<Self> {
        // Same environment policy as the runner: only allowlisted variables reach the server.
        let mut child = Command::new(program)
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .envs(sanitize_env(
                std::env::vars(),
                &tpt_lean_core::policy::default_env_allowlist(),
            ))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| {
                Error::new(
                    ErrorCode::LeanNotFound,
                    format!("cannot start {}: {e}", program.display()),
                )
            })?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");
        let (tx, rx) = channel();
        thread::spawn(move || read_loop(stdout, tx));
        let mut client = Self {
            child,
            stdin,
            messages: rx,
            next_id: 1,
            pending: Vec::new(),
            abandoned: HashSet::new(),
            diagnostics: Default::default(),
            versions: Default::default(),
            progress: HashMap::new(),
            server_version: None,
        };
        let root_uri = path_to_uri(cwd);
        let result = client.request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "rootUri": root_uri,
                "capabilities": {},
                "clientInfo": { "name": "tpt-lean" }
            }),
            timeout,
        )?;
        // Recorded for version reports; features are not gated on it yet.
        client.server_version = result["serverInfo"]["version"].as_str().map(str::to_string);
        client.notify("initialized", json!({}))?;
        Ok(client)
    }

    /// Version string from the initialize result (`serverInfo.version`), when the server sent one.
    pub fn server_version(&self) -> Option<&str> {
        self.server_version.as_deref()
    }

    /// False once the server process has exited.
    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Kill the server process. Further requests fail until the client is replaced.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    pub fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
    }

    /// Send a request and wait up to `timeout` for its response.
    pub fn request(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        self.request_inner(method, params, timeout, None)
    }

    /// Like [`LspClient::request`], but returns early with a cancelled error once `cancel` is set.
    /// A `$/cancelRequest` notification is sent to the server for the abandoned request.
    pub fn request_cancellable(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
        cancel: &CancelToken,
    ) -> Result<Value> {
        self.request_inner(method, params, timeout, Some(cancel))
    }

    fn request_inner(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
        cancel: Option<&CancelToken>,
    ) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))?;
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(pos) = self.pending.iter().position(|m| m.get("id") == Some(&json!(id))) {
                let msg = self.pending.remove(pos);
                return response_result(&msg, method);
            }
            if cancel.is_some_and(CancelToken::is_cancelled) {
                self.abandoned.insert(id);
                self.send(&json!({ "jsonrpc": "2.0", "method": "$/cancelRequest", "params": { "id": id } }))?;
                return Err(Error::new(ErrorCode::Internal, CANCELLED_MESSAGE));
            }
            let now = Instant::now();
            if now >= deadline {
                self.abandoned.insert(id);
                return Err(timeout_error(method, timeout));
            }
            let mut wait = deadline - now;
            if cancel.is_some() {
                wait = wait.min(CANCEL_POLL);
            }
            match self.messages.recv_timeout(wait) {
                Ok(msg) => {
                    if msg.get("id").is_some() && msg.get("method").is_some() {
                        // Server-to-client request: answer with null so it does not block.
                        let reply =
                            json!({ "jsonrpc": "2.0", "id": msg["id"].clone(), "result": Value::Null });
                        self.send(&reply)?;
                    } else if msg.get("id") == Some(&json!(id)) {
                        return response_result(&msg, method);
                    } else if let Some(other) = msg.get("id").and_then(Value::as_i64) {
                        if !self.abandoned.remove(&other) {
                            self.pending.push(msg);
                        }
                    } else {
                        self.keep_notification(&msg);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(Error::new(ErrorCode::Io, "language server closed its output"));
                }
            }
        }
    }

    fn keep_notification(&mut self, msg: &Value) {
        match msg.get("method").and_then(Value::as_str) {
            Some("textDocument/publishDiagnostics") => {
                if let (Some(uri), Some(params)) = (msg["params"]["uri"].as_str(), msg.get("params")) {
                    self.diagnostics.insert(uri.to_string(), params.clone());
                }
            }
            Some("$/lean/fileProgress") => {
                if let (Some(uri), Some(items)) = (
                    msg["params"]["textDocument"]["uri"].as_str(),
                    msg["params"]["processing"].as_array(),
                ) {
                    self.progress.insert(uri.to_string(), items.len());
                }
            }
            _ => {}
        }
    }

    /// Drop the diagnostics and progress recorded for `uri`, so that the next wait sees only what
    /// the server reports for the text it has now. Used when a document changes.
    pub fn forget_document(&mut self, uri: &str) {
        self.diagnostics.remove(uri);
        self.progress.remove(uri);
    }

    /// Work items Lean still reports as processing for `uri`, or `None` when no progress has
    /// been reported yet. Zero means the server has finished elaborating the document.
    /// Drains queued messages first.
    pub fn file_progress(&mut self, uri: &str) -> Option<usize> {
        while let Ok(msg) = self.messages.try_recv() {
            if msg.get("id").is_some() && msg.get("method").is_some() {
                let reply = json!({ "jsonrpc": "2.0", "id": msg["id"].clone(), "result": Value::Null });
                let _ = self.send(&reply);
            } else {
                self.keep_notification(&msg);
            }
        }
        self.progress.get(uri).copied()
    }

    /// Diagnostics the server has published for `uri` so far (drains queued messages first).
    pub fn diagnostics_for(&mut self, uri: &str) -> Vec<Value> {
        self.published_diagnostics(uri).unwrap_or_default()
    }

    /// Diagnostics from the latest `publishDiagnostics` for `uri`, or `None` when the server has
    /// not published for it yet. Drains queued messages first.
    pub fn published_diagnostics(&mut self, uri: &str) -> Option<Vec<Value>> {
        while let Ok(msg) = self.messages.try_recv() {
            if msg.get("id").is_some() && msg.get("method").is_some() {
                let reply = json!({ "jsonrpc": "2.0", "id": msg["id"].clone(), "result": Value::Null });
                let _ = self.send(&reply);
            } else {
                self.keep_notification(&msg);
            }
        }
        self.diagnostics
            .get(uri)
            .map(|p| p["diagnostics"].as_array().cloned().unwrap_or_default())
    }

    /// Open `uri` at `version` with full text (used by `didOpen` and when replaying documents).
    pub(crate) fn open_with_version(&mut self, uri: &str, version: i64, text: &str) -> Result<()> {
        self.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": { "uri": uri, "languageId": "lean4", "version": version, "text": text }
            }),
        )?;
        self.versions.insert(uri.to_string(), version);
        Ok(())
    }

    /// Replace the full text of an open document at `version`.
    pub(crate) fn change_with_version(&mut self, uri: &str, version: i64, text: &str) -> Result<()> {
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": uri, "version": version },
                "contentChanges": [{ "text": text }]
            }),
        )?;
        self.versions.insert(uri.to_string(), version);
        Ok(())
    }

    /// Open the document on first call, otherwise send a full-text change. Returns the URI.
    pub fn sync_document(&mut self, path: &Path, text: &str) -> Result<String> {
        let uri = path_to_uri(path);
        match self.versions.get(&uri).copied() {
            Some(v) => self.change_with_version(&uri, v + 1, text)?,
            None => self.open_with_version(&uri, 1, text)?,
        }
        Ok(uri)
    }

    /// Tell the server a document is closed and forget its diagnostics.
    pub fn close_document(&mut self, path: &Path) -> Result<()> {
        let uri = path_to_uri(path);
        self.versions.remove(&uri);
        self.diagnostics.remove(&uri);
        self.notify("textDocument/didClose", json!({ "textDocument": { "uri": uri } }))
    }

    fn send(&mut self, msg: &Value) -> Result<()> {
        let body = msg.to_string();
        write!(self.stdin, "Content-Length: {}\r\n\r\n{}", body.len(), body)?;
        self.stdin.flush()?;
        Ok(())
    }

    /// Shut the server down cleanly, then make sure the process is gone.
    pub fn shutdown(mut self, timeout: Duration) {
        let _ = self.request("shutdown", Value::Null, timeout);
        let _ = self.notify("exit", Value::Null);
        self.kill();
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        self.kill();
    }
}

fn timeout_error(method: &str, timeout: Duration) -> Error {
    Error::new(
        ErrorCode::Timeout,
        format!("no response to {method} within {timeout:?}"),
    )
}

fn response_result(msg: &Value, method: &str) -> Result<Value> {
    if let Some(err) = msg.get("error") {
        return Err(Error::new(ErrorCode::Internal, format!("{method} failed: {err}")));
    }
    Ok(msg.get("result").cloned().unwrap_or(Value::Null))
}

fn read_loop<R: Read>(stream: R, tx: std::sync::mpsc::Sender<Value>) {
    let mut reader = BufReader::new(stream);
    loop {
        let mut len: Option<usize> = None;
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some(v) = line.strip_prefix("Content-Length:") {
                len = v.trim().parse().ok();
            }
        }
        let Some(n) = len else { continue };
        let mut body = vec![0u8; n];
        if reader.read_exact(&mut body).is_err() {
            return;
        }
        if let Ok(v) = serde_json::from_slice::<Value>(&body) {
            if tx.send(v).is_err() {
                return;
            }
        }
    }
}

/// Params for `$/lean/plainGoal` and `$/lean/plainTermGoal` at a 0-based LSP position.
pub fn plain_goal_params(uri: &str, line: u32, character_utf16: u32) -> Value {
    json!({
        "textDocument": { "uri": uri },
        "position": { "line": line, "character": character_utf16 }
    })
}

/// Goals from a `$/lean/plainGoal` result. Empty when Lean reports no goal.
pub fn parse_plain_goals(result: &Value) -> Vec<String> {
    match result.get("goals").and_then(Value::as_array) {
        Some(goals) => goals
            .iter()
            .filter_map(|g| g.as_str().map(str::to_string))
            .collect(),
        None => Vec::new(),
    }
}

/// Goal from a `$/lean/plainTermGoal` result, or `None` when there is no term goal there.
pub fn parse_plain_term_goal(result: &Value) -> Option<String> {
    result.get("goal").and_then(Value::as_str).map(str::to_string)
}

/// Plain-text goals at a 0-based LSP position, using `$/lean/plainGoal`.
/// Returns an empty list when Lean reports no goal there.
pub fn plain_goals(
    client: &mut LspClient,
    uri: &str,
    line: u32,
    character_utf16: u32,
    timeout: Duration,
) -> Result<Vec<String>> {
    let result = client.request(
        "$/lean/plainGoal",
        plain_goal_params(uri, line, character_utf16),
        timeout,
    )?;
    Ok(parse_plain_goals(&result))
}

/// Plain-text goal of the term at a 0-based LSP position, using `$/lean/plainTermGoal`.
pub fn plain_term_goal(
    client: &mut LspClient,
    uri: &str,
    line: u32,
    character_utf16: u32,
    timeout: Duration,
) -> Result<Option<String>> {
    let result = client.request(
        "$/lean/plainTermGoal",
        plain_goal_params(uri, line, character_utf16),
        timeout,
    )?;
    Ok(parse_plain_term_goal(&result))
}

/// Open a document with its current text so the server elaborates it.
pub fn open_document(client: &mut LspClient, path: &Path, text: &str) -> Result<String> {
    let uri = path_to_uri(path);
    client.open_with_version(&uri, 1, text)?;
    Ok(uri)
}

/// Resolve the program and args for a language server rooted at `project`.
/// Prefers `lake serve` inside a Lake project, otherwise `lean --server`.
pub fn server_command(
    project: &Path,
    lake: Option<&PathBuf>,
    lean: Option<&PathBuf>,
) -> Result<(PathBuf, Vec<String>)> {
    let has_lakefile = project.join("lakefile.lean").is_file() || project.join("lakefile.toml").is_file();
    match (has_lakefile, lake, lean) {
        (true, Some(lake), _) => Ok((lake.clone(), vec!["serve".into()])),
        (_, _, Some(lean)) => Ok((lean.clone(), vec!["--server".into()])),
        _ => Err(Error::new(
            ErrorCode::LeanNotFound,
            "neither lake nor lean is available",
        )),
    }
}
