// SPDX-License-Identifier: MIT OR Apache-2.0
//! Local JSON-RPC 2.0 API server. Transport: newline-delimited JSON over TCP.
//!
//! Local-first: binds 127.0.0.1 unless `server.allow_remote = true`. Every path parameter is
//! resolved through `WorkspaceRoot`. Methods:
//! `project.info`, `index.status`, `search.query`, `format.text`.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use tpt_lean_core::config;
use tpt_lean_core::dirs::ProjectDirs;
use tpt_lean_core::paths::WorkspaceRoot;
use tpt_lean_core::project;
use tpt_lean_core::Result;
use tpt_lean_index::Index;
use tpt_lean_search::SearchOptions;

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const APP_ERROR: i64 = -32000;

/// A server bound to one project.
pub struct Server {
    root: PathBuf,
    workspace: WorkspaceRoot,
    allow_remote: bool,
}

impl Server {
    pub fn new(root: &Path) -> Result<Self> {
        let workspace = WorkspaceRoot::new(root)?;
        let (cfg, _) = config::load_config(workspace.path(), &[])?;
        Ok(Self {
            root: workspace.path().to_path_buf(),
            allow_remote: cfg.server.allow_remote,
            workspace,
        })
    }

    /// Address to bind for this configuration.
    pub fn bind_host(&self) -> &'static str {
        if self.allow_remote {
            "0.0.0.0"
        } else {
            "127.0.0.1"
        }
    }

    /// Handle one JSON-RPC request object. Notifications (no `id`) get no reply.
    pub fn handle(&self, msg: &Value) -> Option<Value> {
        let id = msg.get("id").cloned();
        if msg.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Some(error(
                id.unwrap_or(Value::Null),
                INVALID_REQUEST,
                "jsonrpc must be \"2.0\"",
            ));
        }
        let Some(method) = msg.get("method").and_then(Value::as_str) else {
            return Some(error(
                id.unwrap_or(Value::Null),
                INVALID_REQUEST,
                "missing method",
            ));
        };
        let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));
        let result = match method {
            "project.info" => self.project_info(),
            "index.status" => self.index_status(),
            "search.query" => self.search_query(&params),
            "format.text" => self.format_text(&params),
            _ => Err((METHOD_NOT_FOUND, format!("unknown method {method}"))),
        };
        let id = id?;
        Some(match result {
            Ok(v) => json!({ "jsonrpc": "2.0", "id": id, "result": v }),
            Err((code, message)) => error(id, code, &message),
        })
    }

    fn project_info(&self) -> std::result::Result<Value, (i64, String)> {
        let marker = project::discover(&self.root).ok().map(|p| p.marker);
        Ok(json!({
            "root": self.root.display().to_string(),
            "marker": marker,
            "methods": ["project.info", "index.status", "search.query", "format.text"],
        }))
    }

    fn open_index(&self) -> std::result::Result<Index, (i64, String)> {
        let dirs = ProjectDirs::new(&self.root);
        if !dirs.index_db().exists() {
            return Err((APP_ERROR, "no index yet; run `tpt-lean index scan`".into()));
        }
        Index::open(&dirs.index_db()).map_err(|e| (APP_ERROR, e.to_string()))
    }

    fn index_status(&self) -> std::result::Result<Value, (i64, String)> {
        let s = self
            .open_index()?
            .status()
            .map_err(|e| (APP_ERROR, e.to_string()))?;
        Ok(json!({
            "schema_version": s.schema_version,
            "files": s.files,
            "symbols": s.symbols,
            "packages": s.packages,
            "integrity_ok": s.integrity_ok,
        }))
    }

    fn search_query(&self, params: &Value) -> std::result::Result<Value, (i64, String)> {
        let query = str_param(params, "query")?;
        let limit = params
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(10)
            .clamp(1, 200) as usize;
        let index = self.open_index()?;
        let opts = SearchOptions {
            limit,
            ..Default::default()
        };
        let hits = tpt_lean_search::search(&index, &query, &opts).map_err(|e| (APP_ERROR, e.to_string()))?;
        let items: Vec<Value> = hits
            .into_iter()
            .map(|h| {
                json!({
                    "name": h.name,
                    "kind": h.kind,
                    "source": h.file,
                    "line": h.line,
                    "score": (h.score * 1000.0).round() / 1000.0,
                })
            })
            .collect();
        Ok(json!({ "items": items }))
    }

    fn format_text(&self, params: &Value) -> std::result::Result<Value, (i64, String)> {
        let text = str_param(params, "text")?;
        let out = tpt_lean_fmt::format_text(&text).map_err(|e| (APP_ERROR, e.to_string()))?;
        Ok(json!({ "formatted": out.output, "changed": out.changed }))
    }

    /// Resolve a path parameter inside the workspace. Used by future file-based methods.
    pub fn resolve_path(&self, rel: &str) -> std::result::Result<PathBuf, (i64, String)> {
        self.workspace
            .resolve(rel)
            .map_err(|e| (APP_ERROR, e.to_string()))
    }
}

fn str_param(params: &Value, key: &str) -> std::result::Result<String, (i64, String)> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| (INVALID_PARAMS, format!("missing string param `{key}`")))
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Serve one connection: one JSON-RPC request per line, one response per line.
pub fn serve_connection(server: &Server, stream: TcpStream) -> std::io::Result<()> {
    let reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => server.handle(&msg),
            Err(e) => Some(error(Value::Null, PARSE_ERROR, &e.to_string())),
        };
        if let Some(reply) = reply {
            writeln!(writer, "{reply}")?;
        }
    }
    Ok(())
}

/// Bind and serve forever, one thread per connection. `port` 0 picks a free port.
/// Blocks until the listener fails.
pub fn serve(server: Server, port: u16) -> std::io::Result<()> {
    let listener = TcpListener::bind((server.bind_host(), port))?;
    let server = Arc::new(server);
    for stream in listener.incoming() {
        let stream = stream?;
        let server = Arc::clone(&server);
        thread::spawn(move || {
            let _ = serve_connection(&server, stream);
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::Shutdown;

    fn server() -> Server {
        let dir = std::env::temp_dir().join(format!("tpt-server-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Server::new(&dir).unwrap()
    }

    #[test]
    fn remote_access_is_off_by_default() {
        assert_eq!(server().bind_host(), "127.0.0.1");
    }

    #[test]
    fn rejects_bad_version_and_unknown_method() {
        let s = server();
        let r = s
            .handle(&json!({"jsonrpc":"1.0","id":1,"method":"project.info"}))
            .unwrap();
        assert_eq!(r["error"]["code"], INVALID_REQUEST);
        let r = s
            .handle(&json!({"jsonrpc":"2.0","id":2,"method":"nope"}))
            .unwrap();
        assert_eq!(r["error"]["code"], METHOD_NOT_FOUND);
    }

    #[test]
    fn notifications_have_no_reply() {
        assert!(server()
            .handle(&json!({"jsonrpc":"2.0","method":"project.info"}))
            .is_none());
    }

    #[test]
    fn format_text_method() {
        let r = server()
            .handle(&json!({"jsonrpc":"2.0","id":3,"method":"format.text","params":{"text":"x  \n"}}))
            .unwrap();
        assert_eq!(r["result"]["formatted"], "x\n");
    }

    #[test]
    fn missing_param_is_invalid_params() {
        let r = server()
            .handle(&json!({"jsonrpc":"2.0","id":4,"method":"format.text","params":{}}))
            .unwrap();
        assert_eq!(r["error"]["code"], INVALID_PARAMS);
    }

    #[test]
    fn tcp_round_trip() {
        let s = Arc::new(server());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let s2 = Arc::clone(&s);
        thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            serve_connection(&s2, stream).unwrap();
        });
        let mut client = TcpStream::connect(addr).unwrap();
        client
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"project.info\"}\n")
            .unwrap();
        client.shutdown(Shutdown::Write).unwrap();
        let mut out = String::new();
        client.read_to_string(&mut out).unwrap();
        let v: Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
        assert_eq!(v["id"], 9);
        assert!(v["result"]["methods"].is_array());
    }
}
