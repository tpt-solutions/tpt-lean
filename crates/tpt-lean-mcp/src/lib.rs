// SPDX-License-Identifier: MIT OR Apache-2.0
//! MCP (Model Context Protocol) server over stdio, hand-written on JSON-RPC 2.0 and serde_json.
//!
//! Read-only by default. Tools:
//! - `tpt_lean_search`: search the local index
//! - `tpt_lean_format_text`: format text in memory (never writes files)
//! - `tpt_lean_index_status`: index health and size
//! - `tpt_lean_check_file`: run `lean` on a workspace file (requires `security.allow_build`)
//!
//! Every path argument is resolved through `WorkspaceRoot`, so it must stay inside the project.

use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use tpt_lean_adapter::{self as adapter, Command as LeanCommand};
use tpt_lean_core::config;
use tpt_lean_core::dirs::ProjectDirs;
use tpt_lean_core::paths::WorkspaceRoot;
use tpt_lean_core::policy::CancelToken;
use tpt_lean_core::Result;
use tpt_lean_index::Index;
use tpt_lean_search::SearchOptions;

/// Protocol revision this server implements.
pub const PROTOCOL_VERSION: &str = "2024-11-05";

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// A server bound to one project root.
pub struct Server {
    root: PathBuf,
    workspace: WorkspaceRoot,
    allow_build: bool,
    timeout_seconds: u64,
}

impl Server {
    pub fn new(root: &Path) -> Result<Self> {
        let workspace = WorkspaceRoot::new(root)?;
        let (cfg, _) = config::load_config(workspace.path(), &[])?;
        Ok(Self {
            root: workspace.path().to_path_buf(),
            allow_build: cfg.security.allow_build,
            timeout_seconds: cfg.security.command_timeout_seconds,
            workspace,
        })
    }

    /// Handle one JSON-RPC message. Returns `None` for notifications.
    pub fn handle(&self, msg: &Value) -> Option<Value> {
        let id = msg.get("id").cloned();
        let Some(method) = msg.get("method").and_then(Value::as_str) else {
            return id.map(|id| error_response(id, INVALID_REQUEST, "missing method"));
        };
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let result = match method {
            "initialize" => Ok(json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "tpt-lean", "version": env!("CARGO_PKG_VERSION") }
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tool_descriptors() })),
            "tools/call" if params.get("name").and_then(Value::as_str).is_none() => {
                Err((INVALID_PARAMS, "tools/call needs a tool name".to_string()))
            }
            "tools/call" => Ok(self.call_tool(&params)),
            m if m.starts_with("notifications/") => return None,
            _ => Err((METHOD_NOT_FOUND, format!("unknown method {method}"))),
        };
        let id = id?;
        Some(match result {
            Ok(v) => json!({ "jsonrpc": "2.0", "id": id, "result": v }),
            Err((code, message)) => error_response(id, code, &message),
        })
    }

    fn call_tool(&self, params: &Value) -> Value {
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
        let outcome = match name {
            "tpt_lean_search" => self.tool_search(&args),
            "tpt_lean_format_text" => self.tool_format_text(&args),
            "tpt_lean_index_status" => self.tool_index_status(),
            "tpt_lean_check_file" => self.tool_check_file(&args),
            other => Err(format!("unknown tool {other}")),
        };
        match outcome {
            Ok(v) => json!({ "content": [{ "type": "text", "text": v.to_string() }], "isError": false }),
            Err(msg) => json!({ "content": [{ "type": "text", "text": msg }], "isError": true }),
        }
    }

    fn index(&self) -> std::result::Result<Index, String> {
        let dirs = ProjectDirs::new(&self.root);
        if !dirs.index_db().exists() {
            return Err("no index yet; run `tpt-lean index scan` first".into());
        }
        Index::open(&dirs.index_db()).map_err(|e| e.to_string())
    }

    fn tool_search(&self, args: &Value) -> std::result::Result<Value, String> {
        let query = arg_str(args, "query")?;
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(10)
            .clamp(1, 200) as usize;
        let opts = SearchOptions {
            limit,
            namespace: args.get("namespace").and_then(Value::as_str).map(str::to_string),
            ..Default::default()
        };
        let index = self.index()?;
        let hits = tpt_lean_search::search(&index, &query, &opts).map_err(|e| e.to_string())?;
        let results: Vec<Value> = hits
            .into_iter()
            .map(|h| {
                json!({
                    "name": h.name,
                    "kind": h.kind,
                    "package": h.package,
                    "source": h.file,
                    "line": h.line,
                    "score": (h.score * 1000.0).round() / 1000.0,
                    "docstring": h.docstring,
                })
            })
            .collect();
        Ok(json!({ "results": results }))
    }

    fn tool_format_text(&self, args: &Value) -> std::result::Result<Value, String> {
        let text = arg_str(args, "text")?;
        match tpt_lean_fmt::format_text(&text) {
            Ok(out) => Ok(json!({ "formatted": out.output, "changed": out.changed })),
            Err(e) => Err(e.to_string()),
        }
    }

    fn tool_index_status(&self) -> std::result::Result<Value, String> {
        let status = self.index()?.status().map_err(|e| e.to_string())?;
        Ok(json!({
            "schema_version": status.schema_version,
            "files": status.files,
            "symbols": status.symbols,
            "imports": status.imports,
            "packages": status.packages,
            "integrity_ok": status.integrity_ok,
        }))
    }

    fn tool_check_file(&self, args: &Value) -> std::result::Result<Value, String> {
        if !self.allow_build {
            return Err("building is disabled by security.allow_build".into());
        }
        let file = arg_str(args, "file")?;
        let path = self.workspace.resolve(&file).map_err(|e| e.to_string())?;
        let timeout = args
            .get("timeout_seconds")
            .and_then(Value::as_u64)
            .unwrap_or(self.timeout_seconds)
            .min(self.timeout_seconds.max(1));
        let tc = adapter::discover_toolchain();
        let policy = adapter::lean_policy(timeout);
        let result = adapter::execute(
            &tc,
            &LeanCommand::LeanCheck { file: path },
            Some(&self.root),
            &policy,
            &CancelToken::new(),
        )
        .map_err(|e| e.to_string())?;
        let diagnostics = serde_json::to_value(&result.diagnostics).map_err(|e| e.to_string())?;
        Ok(json!({
            "success": result.output.success(),
            "timed_out": result.output.timed_out,
            "diagnostics": diagnostics,
        }))
    }
}

fn arg_str(args: &Value, key: &str) -> std::result::Result<String, String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("missing string argument `{key}`"))
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Tool metadata returned by `tools/list`.
pub fn tool_descriptors() -> Value {
    json!([
        {
            "name": "tpt_lean_search",
            "description": "Search the local Lean index by name, prefix, or docstring text.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 200 },
                    "namespace": { "type": "string" }
                },
                "required": ["query"]
            }
        },
        {
            "name": "tpt_lean_format_text",
            "description": "Format Lean source text in memory. Never writes files.",
            "inputSchema": {
                "type": "object",
                "properties": { "text": { "type": "string" } },
                "required": ["text"]
            }
        },
        {
            "name": "tpt_lean_index_status",
            "description": "Report index schema version, sizes, packages, and integrity.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "tpt_lean_check_file",
            "description": "Run `lean` on a file inside the workspace and return diagnostics. Requires build permission.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file": { "type": "string" },
                    "timeout_seconds": { "type": "integer", "minimum": 1 }
                },
                "required": ["file"]
            }
        }
    ])
}

/// Serve JSON-RPC over newline-delimited stdio until EOF.
pub fn serve_stdio<R: BufRead, W: Write>(server: &Server, input: R, mut output: W) -> std::io::Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => server.handle(&msg),
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
    fn initialize_reports_tools_capability() {
        let s = server();
        let r = s
            .handle(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}))
            .unwrap();
        assert_eq!(r["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert!(r["result"]["capabilities"]["tools"].is_object());
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
                "tpt_lean_check_file"
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
}
