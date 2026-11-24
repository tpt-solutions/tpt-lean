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
pub const PROTOCOL_VERSION: &str = "2024-11-05";

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

        let key = id.to_string();
        let token = CancelToken::new();
        if !self.begin_request(&key, &token) {
            return None;
        }
        let result = self.dispatch(method, &params, &token);
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
    ) -> std::result::Result<Value, RpcError> {
        match method {
            "initialize" => Ok(json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": {}, "resources": {}, "prompts": {} },
                "serverInfo": { "name": "tpt-lean", "version": env!("CARGO_PKG_VERSION") }
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tool_descriptors() })),
            "tools/call" => {
                let name = str_param(params, "name")
                    .ok_or((INVALID_PARAMS, "tools/call needs a tool name".to_string()))?;
                if tools::find(&name).is_none() {
                    return Err((INVALID_PARAMS, format!("unknown tool {name}")));
                }
                Ok(self.call_tool(&name, params, cancel))
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

pub(crate) fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Serve JSON-RPC over newline-delimited stdio until EOF.
///
/// Requests are answered in order on the calling thread. The loop is synchronous, so a
/// cancellation can only refer to a request that has already been answered; it is accepted
/// and has no effect. In-flight cancellation applies to the HTTP transport.
pub fn serve_stdio<R: BufRead, W: Write>(server: &Server, input: R, mut output: W) -> std::io::Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => {
                if let Some(id) = msg.get("id").filter(|_| msg.get("method").is_some()) {
                    server.note_received(&id.to_string());
                }
                server.handle(&msg)
            }
            Err(e) => Some(error_response(Value::Null, PARSE_ERROR, &e.to_string())),
        };
        if let Some(reply) = reply {
            writeln!(output, "{reply}")?;
            output.flush()?;
        }
    }
    Ok(())
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
