// SPDX-License-Identifier: MIT OR Apache-2.0
//! MCP (Model Context Protocol) server, hand-written on JSON-RPC 2.0 and serde_json.
//!
//! Transports: newline-delimited stdio ([`serve_stdio`]) and a loopback-only HTTP transport
//! with server-sent events ([`serve_http`]). Protocol revision: [`PROTOCOL_VERSION`].
//!
//! Read-only by default. Tools declare a permission class (see [`Permission`]); the server
//! refuses a call whose class the project config does not grant. Every path argument goes
//! through `WorkspaceRoot`, so it must stay inside the project.

mod http;
mod lean;
mod permission;
mod prompts;
mod resources;
mod tools;

pub use http::serve_http;
pub use permission::Permission;
pub use tools::tool_descriptors;

use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use tpt_lean_core::config::{self, Config};
use tpt_lean_core::paths::WorkspaceRoot;
use tpt_lean_core::policy::CancelToken;
use tpt_lean_core::Result;

/// Protocol revision this server implements.
use std::collections::VecDeque;
use std::sync::mpsc::{self, TryRecvError};
use std::thread;
use std::time::Duration;

/// The revision the server negotiates when a client names none (the newest one that uses the
/// `initialize` handshake).
pub const PROTOCOL_VERSION: &str = "2025-11-25";

/// The newest revision the server knows. It has no `initialize` handshake: capabilities come from
/// `server/discover`, and every request names its version in `_meta`.
pub const LATEST_PROTOCOL_VERSION: &str = "2026-07-28";

/// Every revision the server speaks, newest first. Features are the same in each: tools, resources,
/// prompts, progress and cancellation. Tasks and subscriptions are not implemented.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &[
    "2026-07-28",
    "2025-11-25",
    "2025-06-18",
    "2025-03-26",
    "2024-11-05",
];

/// `_meta` key that carries the protocol version of each request in the newest revision.
const META_PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";

/// Error code for a request whose protocol version the server does not support.
pub(crate) const UNSUPPORTED_PROTOCOL_VERSION: i64 = -32022;

/// Instructions returned with `initialize` and `server/discover`.
const INSTRUCTIONS: &str = "tpt-lean: Lean project tools. Read tools search the index and format text; \
build and profile tools run Lake and Lean and need the build or profile permission. Resources \
expose project info, cached diagnostics, the index status, file symbols and syntax (ast), goals \
at a position, the last build log and stored profiles.";

pub(crate) const PARSE_ERROR: i64 = -32700;
pub(crate) const INVALID_REQUEST: i64 = -32600;
pub(crate) const METHOD_NOT_FOUND: i64 = -32601;
pub(crate) const INVALID_PARAMS: i64 = -32602;
pub(crate) const RESOURCE_NOT_FOUND: i64 = -32002;
pub(crate) const INTERNAL_ERROR: i64 = -32603;

/// A JSON-RPC error: code and message.
pub(crate) type RpcError = (i64, String);

/// Request bookkeeping for `notifications/cancelled`.
#[derive(Default)]
struct Requests {
    /// Request ids received and not yet answered.
    pending: HashSet<String>,
    /// Pending ids the client has cancelled. Their results are dropped.
    cancelled: HashSet<String>,
    /// Cancellation tokens of requests currently running.
    inflight: HashMap<String, CancelToken>,
}

/// A server bound to one project root.
pub struct Server {
    root: PathBuf,
    workspace: WorkspaceRoot,
    config: Config,
    /// Latest diagnostics per workspace-relative file, from check and build runs.
    diagnostics: Mutex<BTreeMap<String, Vec<Value>>>,
    requests: Mutex<Requests>,
    /// Result of the latest `tpt_lean_build_project` call in this process.
    last_build: Mutex<Option<Value>>,
}

impl Server {
    /// Load the project config under `root` and bind the server to it.
    pub fn new(root: &Path) -> Result<Self> {
        let workspace = WorkspaceRoot::new(root)?;
        let (config, _) = config::load_config(workspace.path(), &[])?;
        Ok(Self {
            root: workspace.path().to_path_buf(),
            workspace,
            config,
            diagnostics: Mutex::new(BTreeMap::new()),
            requests: Mutex::new(Requests::default()),
            last_build: Mutex::new(None),
        })
    }

    /// The effective project configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The canonical project root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn workspace(&self) -> &WorkspaceRoot {
        &self.workspace
    }

    /// Handle one JSON-RPC message. Returns `None` for notifications and for requests whose
    /// client cancelled them before the reply was due.
    pub fn handle(&self, msg: &Value) -> Option<Value> {
        self.handle_with(msg, &mut |_| {})
    }

    /// Like [`Server::handle`]. While a tool call that carries a `_meta.progressToken` runs, its
    /// `notifications/progress` messages go to `progress` before the reply.
    pub fn handle_with(&self, msg: &Value, progress: &mut dyn FnMut(Value)) -> Option<Value> {
        if msg.is_array() {
            return Some(error_response(
                Value::Null,
                INVALID_REQUEST,
                "batch requests are not supported",
            ));
        }
        let Some(obj) = msg.as_object() else {
            return Some(error_response(
                Value::Null,
                INVALID_REQUEST,
                "message must be a JSON object",
            ));
        };
        let id = obj.get("id").cloned();
        let Some(method) = obj.get("method").and_then(Value::as_str) else {
            // A response to a request we never sent (we make no server-to-client requests).
            if obj.contains_key("result") || obj.contains_key("error") {
                return None;
            }
            return Some(error_response(
                id.unwrap_or(Value::Null),
                INVALID_REQUEST,
                "missing method",
            ));
        };
        let params = obj.get("params").cloned().unwrap_or(Value::Null);
        let Some(id) = id else {
            self.notification(method, &params);
            return None;
        };

        // The newest revision names its version in every request. Refuse versions we do not speak,
        // and say which ones we do.
        if let Some(requested) = params
            .get("_meta")
            .and_then(|m| m.get(META_PROTOCOL_VERSION))
            .and_then(Value::as_str)
        {
            if !SUPPORTED_PROTOCOL_VERSIONS.contains(&requested) {
                return Some(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {
                        "code": UNSUPPORTED_PROTOCOL_VERSION,
                        "message": format!("unsupported protocol version {requested}"),
                        "data": { "supported": SUPPORTED_PROTOCOL_VERSIONS, "requested": requested }
                    }
                }));
            }
        }

        let key = id.to_string();
        let token = CancelToken::new();
        if !self.begin_request(&key, &token) {
            return None;
        }
        let result = self.dispatch(method, &params, &token, progress);
        if self.finish_request(&key) {
            return None;
        }
        Some(match result {
            Ok(v) => json!({ "jsonrpc": "2.0", "id": id, "result": v }),
            Err((code, message)) => error_response(id, code, &message),
        })
    }

    fn notification(&self, method: &str, params: &Value) {
        if method == "notifications/cancelled" {
            if let Some(id) = params.get("requestId") {
                self.cancel_request(&id.to_string());
            }
        }
    }

    /// Record that a request with this key (the JSON text of its id) has been received.
    /// Transports call this before they hand a request to [`Server::handle`], so a cancel
    /// that arrives while the request is queued is still honored.
    pub(crate) fn note_received(&self, id_key: &str) {
        self.lock_requests().pending.insert(id_key.to_string());
    }

    /// Mark a pending request as cancelled and stop it if it is running.
    pub(crate) fn cancel_request(&self, id_key: &str) {
        let mut reqs = self.lock_requests();
        if reqs.pending.contains(id_key) {
            reqs.cancelled.insert(id_key.to_string());
            if let Some(token) = reqs.inflight.get(id_key) {
                token.cancel();
            }
        }
    }

    /// Returns false when the request was cancelled before it started; its bookkeeping is
    /// cleared so the id can be used again.
    fn begin_request(&self, id_key: &str, token: &CancelToken) -> bool {
        let mut reqs = self.lock_requests();
        reqs.pending.insert(id_key.to_string());
        if reqs.cancelled.remove(id_key) {
            reqs.pending.remove(id_key);
            return false;
        }
        reqs.inflight.insert(id_key.to_string(), token.clone());
        true
    }

    /// Forget a finished request. Returns true when it was cancelled, so its reply is dropped.
    fn finish_request(&self, id_key: &str) -> bool {
        let mut reqs = self.lock_requests();
        reqs.inflight.remove(id_key);
        reqs.pending.remove(id_key);
        reqs.cancelled.remove(id_key)
    }

    fn lock_requests(&self) -> MutexGuard<'_, Requests> {
        self.requests.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn dispatch(
        &self,
        method: &str,
        params: &Value,
        cancel: &CancelToken,
        progress: &mut dyn FnMut(Value),
    ) -> std::result::Result<Value, RpcError> {
        match method {
            "initialize" => {
                // Answer with the requested revision when we speak it, and otherwise with ours.
                let requested = str_param(params, "protocolVersion");
                let version = requested
                    .as_deref()
                    .and_then(|r| SUPPORTED_PROTOCOL_VERSIONS.iter().find(|v| **v == r))
                    .copied()
                    .unwrap_or(PROTOCOL_VERSION);
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": capabilities(),
                    "serverInfo": { "name": "tpt-lean", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": INSTRUCTIONS,
                }))
            }
            "server/discover" => Ok(json!({
                "supportedVersions": SUPPORTED_PROTOCOL_VERSIONS,
                "capabilities": capabilities(),
                "instructions": INSTRUCTIONS,
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tool_descriptors() })),
            "tools/call" => {
                let name = str_param(params, "name")
                    .ok_or((INVALID_PARAMS, "tools/call needs a tool name".to_string()))?;
                if tools::find(&name).is_none() {
                    return Err((INVALID_PARAMS, format!("unknown tool {name}")));
                }
                // With a progress token, report that the call started and that it finished. A
                // single tool call has no finer steps to report.
                let token = params.get("_meta").and_then(|m| m.get("progressToken")).cloned();
                if let Some(token) = &token {
                    progress(progress_note(token, 0.0, 1.0, &format!("running {name}")));
                }
                let result = self.call_tool(&name, params, cancel);
                if let Some(token) = &token {
                    progress(progress_note(token, 1.0, 1.0, &format!("{name} finished")));
                }
                Ok(result)
            }
            "resources/list" => Ok(resources::list()),
            "resources/templates/list" => Ok(resources::templates()),
            "resources/read" => {
                let uri = str_param(params, "uri")
                    .ok_or((INVALID_PARAMS, "resources/read needs a uri".to_string()))?;
                resources::read(self, &uri)
            }
            "prompts/list" => Ok(prompts::list()),
            "prompts/get" => prompts::get(params),
            _ => Err((METHOD_NOT_FOUND, format!("unknown method {method}"))),
        }
    }

    /// Remember the result of the latest build, for the build-log resource.
    pub(crate) fn remember_build(&self, result: Value) {
        *self.last_build.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
    }

    /// The result of the latest build in this process, if one ran.
    pub(crate) fn last_build(&self) -> Option<Value> {
        self.last_build.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Run a tool after its permission check. Failures are tool errors (`isError: true`),
    /// not protocol errors, so the client can show them to the model.
    fn call_tool(&self, name: &str, params: &Value, cancel: &CancelToken) -> Value {
        let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
        let Some(spec) = tools::find(name) else {
            return tool_error(format!("unknown tool {name}"));
        };
        let outcome = permission::check(&self.config.security, spec.class)
            .and_then(|()| self.run_tool(name, &args, cancel));
        match outcome {
            Ok(v) => json!({
                "content": [{ "type": "text", "text": v.to_string() }],
                "structuredContent": v,
                "isError": false
            }),
            Err(msg) => tool_error(msg),
        }
    }

    /// Store the latest diagnostics for one file (an empty list clears it).
    pub(crate) fn store_diagnostics(&self, file: &str, diagnostics: Vec<Value>) {
        let mut map = self.diagnostics.lock().unwrap_or_else(|e| e.into_inner());
        if diagnostics.is_empty() {
            map.remove(file);
        } else {
            map.insert(file.to_string(), diagnostics);
        }
    }

    /// Cached diagnostics per file, in path order.
    pub(crate) fn cached_diagnostics(&self) -> BTreeMap<String, Vec<Value>> {
        self.diagnostics.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

fn tool_error(message: String) -> Value {
    json!({ "content": [{ "type": "text", "text": message }], "isError": true })
}

fn str_param(params: &Value, key: &str) -> Option<String> {
    params.get(key).and_then(Value::as_str).map(str::to_string)
}

/// The capabilities this server offers. None of them change while it runs, so `listChanged` is false
/// everywhere and nothing is subscribable.
fn capabilities() -> Value {
    json!({
        "tools": { "listChanged": false },
        "resources": { "subscribe": false, "listChanged": false },
        "prompts": { "listChanged": false },
    })
}

/// A `notifications/progress` message for `token`.
fn progress_note(token: &Value, progress: f64, total: f64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "notifications/progress",
        "params": { "progressToken": token, "progress": progress, "total": total, "message": message },
    })
}

pub(crate) fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Serve JSON-RPC over newline-delimited stdio until the input ends.
///
/// Replies go out in the order the requests arrived. Each request runs on a worker thread, while
/// the calling thread keeps reading input. A `notifications/cancelled` that arrives during a tool call
/// therefore stops that call. A tool call with a progress token gets `notifications/progress`
/// messages before its reply.
pub fn serve_stdio<R: BufRead + Send, W: Write + Send>(
    server: &Server,
    input: R,
    output: W,
) -> std::io::Result<()> {
    let output = Mutex::new(output);
    let failure: Mutex<Option<std::io::Error>> = Mutex::new(None);
    let (line_tx, line_rx) = mpsc::channel::<std::io::Result<String>>();
    std::thread::scope(|scope| {
        scope.spawn(move || {
            for line in input.lines() {
                let failed = line.is_err();
                if line_tx.send(line).is_err() || failed {
                    break;
                }
            }
        });

        let mut queue: VecDeque<String> = VecDeque::new();
        let mut input_open = true;
        // Signalled (by disconnect) when the worker for the current request has finished.
        let mut busy: Option<mpsc::Receiver<()>> = None;
        loop {
            // Read what is available. A cancellation is applied at once, even while a request runs;
            // everything else waits its turn in the queue.
            while input_open {
                match line_rx.try_recv() {
                    Ok(Ok(line)) => {
                        if line.trim().is_empty() {
                            continue;
                        }
                        if is_cancellation(&line) {
                            if let Ok(msg) = serde_json::from_str::<Value>(&line) {
                                server.handle(&msg);
                            }
                        } else {
                            if let Some(id) = serde_json::from_str::<Value>(&line)
                                .ok()
                                .and_then(|m| m.get("id").filter(|_| m.get("method").is_some()).cloned())
                            {
                                server.note_received(&id.to_string());
                            }
                            queue.push_back(line);
                        }
                    }
                    Ok(Err(e)) => {
                        *failure.lock().unwrap_or_else(|x| x.into_inner()) = Some(e);
                        input_open = false;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => input_open = false,
                }
            }

            if let Some(done) = &busy {
                if !matches!(done.try_recv(), Err(TryRecvError::Empty)) {
                    busy = None;
                }
            }
            if busy.is_none() {
                if let Some(line) = queue.pop_front() {
                    let (done_tx, done_rx) = mpsc::channel::<()>();
                    let output = &output;
                    let failure = &failure;
                    scope.spawn(move || {
                        // Dropping `done_tx` at the end tells the loop the worker has finished.
                        let _done = done_tx;
                        serve_one(server, &line, output, failure);
                    });
                    busy = Some(done_rx);
                }
            }
            if busy.is_none() && queue.is_empty() && !input_open {
                break;
            }
            thread::sleep(Duration::from_millis(2));
        }
    });
    match failure.into_inner().unwrap_or_else(|e| e.into_inner()) {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Whether a line is a `notifications/cancelled` message.
fn is_cancellation(line: &str) -> bool {
    serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|m| {
            m.get("method")
                .and_then(Value::as_str)
                .map(|s| s == "notifications/cancelled")
        })
        .unwrap_or(false)
}

/// Answer one line: its reply, and any progress messages before it.
fn serve_one<W: Write>(
    server: &Server,
    line: &str,
    output: &Mutex<W>,
    failure: &Mutex<Option<std::io::Error>>,
) {
    let reply = match serde_json::from_str::<Value>(line) {
        Ok(msg) => server.handle_with(&msg, &mut |note| write_line(output, failure, &note)),
        Err(e) => Some(error_response(Value::Null, PARSE_ERROR, &e.to_string())),
    };
    if let Some(reply) = reply {
        write_line(output, failure, &reply);
    }
}

/// Write one message and flush. The first write error is kept for [`serve_stdio`] to return.
fn write_line<W: Write>(output: &Mutex<W>, failure: &Mutex<Option<std::io::Error>>, value: &Value) {
    let mut out = output.lock().unwrap_or_else(|e| e.into_inner());
    if let Err(e) = writeln!(out, "{value}").and_then(|()| out.flush()) {
        let mut slot = failure.lock().unwrap_or_else(|x| x.into_inner());
        if slot.is_none() {
            *slot = Some(e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> Server {
        let dir = std::env::temp_dir().join(format!("tpt-mcp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Server::new(&dir).unwrap()
    }

    #[test]
    fn initialize_negotiates_the_requested_version() {
        let s = server();
        let r = s
            .handle(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}))
            .unwrap();
        assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
        // A version we do not speak gets the server's default, which the client may then reject.
        let r = s
            .handle(&json!({"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2099-01-01"}}))
            .unwrap();
        assert_eq!(r["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert!(r["result"]["capabilities"]["tools"].is_object());
        assert!(r["result"]["instructions"].as_str().unwrap().contains("tpt-lean"));
    }

    #[test]
    fn server_discover_lists_every_supported_revision() {
        let r = server()
            .handle(&json!({"jsonrpc":"2.0","id":3,"method":"server/discover","params":{}}))
            .unwrap();
        let versions: Vec<&str> = r["result"]["supportedVersions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(versions, SUPPORTED_PROTOCOL_VERSIONS);
        assert_eq!(versions[0], LATEST_PROTOCOL_VERSION);
        assert_eq!(r["result"]["capabilities"]["prompts"]["listChanged"], false);
    }

    #[test]
    fn a_request_for_an_unsupported_version_is_refused_with_the_supported_list() {
        let s = server();
        let meta = json!({ "io.modelcontextprotocol/protocolVersion": "2099-01-01" });
        let r = s
            .handle(&json!({"jsonrpc":"2.0","id":4,"method":"ping","params":{"_meta": meta}}))
            .unwrap();
        assert_eq!(r["error"]["code"], UNSUPPORTED_PROTOCOL_VERSION);
        assert_eq!(r["error"]["data"]["requested"], "2099-01-01");
        assert!(r["error"]["data"]["supported"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "2026-07-28"));

        let meta = json!({ "io.modelcontextprotocol/protocolVersion": LATEST_PROTOCOL_VERSION });
        let r = s
            .handle(&json!({"jsonrpc":"2.0","id":5,"method":"ping","params":{"_meta": meta}}))
            .unwrap();
        assert_eq!(r["result"], json!({}));
    }

    #[test]
    fn a_tool_call_with_a_progress_token_reports_progress_before_the_reply() {
        let s = server();
        let mut notes: Vec<Value> = Vec::new();
        let reply = s.handle_with(
            &json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{
                "name":"tpt_lean_format_text",
                "arguments":{"text":"x  
"},
                "_meta":{"progressToken":"tok-1"}}}),
            &mut |n| notes.push(n),
        );
        assert!(reply.unwrap()["result"]["isError"] == false);
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert_eq!(notes[0]["method"], "notifications/progress");
        assert_eq!(notes[0]["params"]["progressToken"], "tok-1");
        assert_eq!(notes[0]["params"]["progress"], 0.0);
        assert_eq!(notes[1]["params"]["progress"], 1.0);

        // Without a token there are no progress messages.
        let mut none: Vec<Value> = Vec::new();
        s.handle_with(
            &json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{
                "name":"tpt_lean_format_text","arguments":{"text":"x
"}}}),
            &mut |n| none.push(n),
        );
        assert!(none.is_empty());
    }

    #[test]
    fn initialize_reports_capabilities() {
        let s = server();
        let r = s
            .handle(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}))
            .unwrap();
        assert_eq!(r["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert!(r["result"]["capabilities"]["tools"].is_object());
        assert!(r["result"]["capabilities"]["resources"].is_object());
        assert!(r["result"]["capabilities"]["prompts"].is_object());
    }

    #[test]
    fn notifications_get_no_reply() {
        let s = server();
        assert!(s
            .handle(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .is_none());
    }

    #[test]
    fn unknown_method_is_error() {
        let s = server();
        let r = s
            .handle(&json!({"jsonrpc":"2.0","id":7,"method":"nope"}))
            .unwrap();
        assert_eq!(r["error"]["code"], METHOD_NOT_FOUND);
        assert_eq!(r["id"], 7);
    }

    #[test]
    fn ping_answers_empty_result() {
        let s = server();
        let r = s
            .handle(&json!({"jsonrpc":"2.0","id":8,"method":"ping"}))
            .unwrap();
        assert_eq!(r["result"], json!({}));
    }

    #[test]
    fn unknown_tool_is_protocol_error() {
        let s = server();
        let r = s
            .handle(&json!({"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"nope"}}))
            .unwrap();
        assert_eq!(r["error"]["code"], INVALID_PARAMS);
    }

    #[test]
    fn tools_list_names_all_tools() {
        let s = server();
        let r = s
            .handle(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}))
            .unwrap();
        let names: Vec<&str> = r["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            vec![
                "tpt_lean_search",
                "tpt_lean_format_text",
                "tpt_lean_index_status",
                "tpt_lean_check_file",
                "tpt_lean_format_file",
                "tpt_lean_goals_at",
                "tpt_lean_explain_diagnostic",
                "tpt_lean_context_pack",
                "tpt_lean_build_project",
                "tpt_lean_profile_file",
                "tpt_lean_imports_suggest",
            ]
        );
    }

    #[test]
    fn format_tool_is_in_memory() {
        let s = server();
        let r = s.handle(&json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"tpt_lean_format_text","arguments":{"text":"def x := 1   \n"}}})).unwrap();
        let body: Value = serde_json::from_str(r["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(body["formatted"], "def x := 1\n");
        assert_eq!(r["result"]["isError"], false);
        assert_eq!(r["result"]["structuredContent"]["changed"], true);
    }

    #[test]
    fn format_tool_refuses_uncertain_input() {
        let s = server();
        let r = s.handle(&json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"tpt_lean_format_text","arguments":{"text":"theorem t := by\n\tsimp\n"}}})).unwrap();
        assert_eq!(r["result"]["isError"], true);
    }

    #[test]
    fn stdio_round_trip_and_parse_error() {
        let s = server();
        let input = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\nnot json\n";
        let mut out = Vec::new();
        serve_stdio(&s, &input[..], &mut out).unwrap();
        let lines: Vec<&str> = std::str::from_utf8(&out).unwrap().lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[1].contains("-32700"));
    }

    #[test]
    fn check_tool_rejects_paths_outside_workspace() {
        let s = server();
        let r = s.handle(&json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"tpt_lean_check_file","arguments":{"file":"../../etc/passwd"}}})).unwrap();
        assert_eq!(r["result"]["isError"], true);
    }

    #[test]
    fn cancelled_pending_request_gets_no_reply() {
        let s = server();
        s.note_received("9");
        s.handle(&json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":9}}));
        assert!(s
            .handle(&json!({"jsonrpc":"2.0","id":9,"method":"ping"}))
            .is_none());
        // Once answered, the id is free again: a later request with the same id is served.
        let r = s
            .handle(&json!({"jsonrpc":"2.0","id":9,"method":"ping"}))
            .unwrap();
        assert_eq!(r["id"], 9);
    }

    #[test]
    fn cancel_for_unknown_id_is_ignored() {
        let s = server();
        s.handle(&json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":77}}));
        let r = s
            .handle(&json!({"jsonrpc":"2.0","id":77,"method":"ping"}))
            .unwrap();
        assert_eq!(r["id"], 77);
    }

    #[test]
    fn batch_requests_are_rejected() {
        let s = server();
        let r = s
            .handle(&json!([{"jsonrpc":"2.0","id":1,"method":"ping"}]))
            .unwrap();
        assert_eq!(r["error"]["code"], INVALID_REQUEST);
    }
}
