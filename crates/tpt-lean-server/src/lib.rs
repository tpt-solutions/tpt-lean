// SPDX-License-Identifier: MIT OR Apache-2.0
//! Local JSON-RPC 2.0 API server. Transport: newline-delimited JSON over a Unix domain socket
//! (Linux and macOS, the default there) or over TCP on 127.0.0.1 (every platform, the default on
//! Windows).
//!
//! - Remote access is off by default (`server.allow_remote`).
//! - Each request is checked against its method's permission class (see [`permission`]).
//! - Every path parameter is resolved through `WorkspaceRoot`; only `.lean` files are accepted.
//! - Method parameters and results are described by JSON Schemas in `schemas/`.
//! - Lifecycle: [`run_foreground`] writes `.tpt-lean/server.json`; [`status`] and [`stop`] read it.
//!
//! Not implemented here: named pipes, SSE event streams. A Unix socket file is created with
//! owner-only permissions (0600), so other local users cannot connect.

mod builds;
pub mod client;
mod context_api;
mod doctor_api;
mod explain_api;
mod imports_api;
mod methods;
pub mod permission;
mod project_api;
pub mod state;
mod warm;

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};
use tpt_lean_adapter::Diagnostic;
use tpt_lean_core::config::{self, Config};
use tpt_lean_core::paths::WorkspaceRoot;
use tpt_lean_core::project;
use tpt_lean_core::{Error, ErrorCode, Result};

pub use client::call;
pub use permission::Permission;
pub use client::Endpoint;
pub use state::{
    run_foreground, run_foreground_on, status, stop, ServerState, StatusReport, StopOutcome,
    StopReport, Transport,
};

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
/// Generic application failure (index missing, Lean failed to start, I/O).
pub const APP_ERROR: i64 = -32000;
/// The method's permission class is not granted by the configuration.
pub const PERMISSION_DENIED: i64 = -32001;
/// A path parameter resolves outside the project root.
pub const PATH_OUTSIDE_WORKSPACE: i64 = -32002;

/// Largest accepted request line, in bytes.
pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;

/// A JSON-RPC error with an optional `data` object.
#[derive(Debug, Clone, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
}

/// Result of a method: a JSON value or an error.
pub type MethodResult = std::result::Result<Value, RpcError>;

/// A server bound to one project.
pub struct Server {
    root: PathBuf,
    workspace: WorkspaceRoot,
    config: Config,
    /// Diagnostics from the most recent `check.file` per file, for this process only.
    diagnostics: Mutex<HashMap<PathBuf, Vec<Diagnostic>>>,
    /// Builds started by `build.run`, for this process only.
    builds: Mutex<builds::Builds>,
    shutdown: AtomicBool,
    /// Endpoint the listener is bound to, used to wake the accept loop on shutdown.
    listen_addr: Mutex<Option<Endpoint>>,
    /// The Lean language server kept warm between requests (see [`warm`]).
    warm: Mutex<warm::WarmState>,
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Server {
    pub fn new(root: &Path) -> Result<Self> {
        let workspace = WorkspaceRoot::new(root)?;
        let (config, _) = config::load_config(workspace.path(), &[])?;
        Ok(Self {
            root: workspace.path().to_path_buf(),
            config,
            workspace,
            diagnostics: Mutex::new(HashMap::new()),
            builds: Mutex::new(builds::Builds::default()),
            shutdown: AtomicBool::new(false),
            listen_addr: Mutex::new(None),
            warm: Mutex::new(warm::WarmState::default()),
        })
    }

    /// Canonical project root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Effective configuration used for permission checks.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Address to bind for this configuration.
    pub fn bind_host(&self) -> &'static str {
        if self.config.server.allow_remote {
            "0.0.0.0"
        } else {
            "127.0.0.1"
        }
    }

    pub fn is_shutdown_requested(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }

    /// Handle one JSON-RPC request object. Notifications (no `id`) get no reply.
    pub fn handle(&self, msg: &Value) -> Option<Value> {
        let id = msg.get("id").cloned();
        if msg.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Some(error_reply(
                id.unwrap_or(Value::Null),
                &RpcError::new(INVALID_REQUEST, "jsonrpc must be \"2.0\""),
            ));
        }
        let Some(method) = msg.get("method").and_then(Value::as_str) else {
            return Some(error_reply(
                id.unwrap_or(Value::Null),
                &RpcError::new(INVALID_REQUEST, "missing method"),
            ));
        };
        let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));
        let result = match permission::lookup(method) {
            None => Err(RpcError::new(
                METHOD_NOT_FOUND,
                format!("unknown method {method}"),
            )),
            Some(needed) if !needed.granted(&self.config) => Err(self.denied(method, needed)),
            Some(_) => self.dispatch(method, &params),
        };
        let id = id?;
        Some(match result {
            Ok(v) => json!({ "jsonrpc": "2.0", "id": id, "result": v }),
            Err(e) => error_reply(id, &e),
        })
    }

    fn denied(&self, method: &str, needed: Permission) -> RpcError {
        let key = needed.config_key().unwrap_or("");
        RpcError {
            code: PERMISSION_DENIED,
            message: format!(
                "{method} needs {} permission; set {key} = true in tpt-lean.toml",
                needed.as_str()
            ),
            data: Some(json!({ "permission": needed.as_str(), "config_key": key })),
        }
    }

    fn dispatch(&self, method: &str, params: &Value) -> MethodResult {
        match method {
            "project.info" => self.project_info(),
            "index.status" => self.index_status(),
            "index.refresh" => self.index_refresh(),
            "search.query" => self.search_query(params),
            "search.symbol" => self.search_symbol(params),
            "format.text" => self.format_text(params),
            "format.file" => self.format_file(params),
            "check.file" => self.check_file(params),
            "profile.run" => self.profile_run(params),
            "profile.get" => self.profile_get(params),
            "build.run" => self.build_run(params),
            "build.status" => self.build_status(params),
            "build.cancel" => self.build_cancel(params),
            "imports.suggest" => self.imports_suggest(params),
            "imports.unused" => self.imports_unused(params),
            "check.project" => self.check_project(params),
            "diagnostics.explain" => self.diagnostics_explain(params),
            "context.pack" => self.context_pack(params),
            "project.doctor" => self.project_doctor(params),
            "search.goal" => self.search_goal(params),
            "diagnostics.get" => self.diagnostics_get(params),
            "goals.at" => self.goals_at(params),
            "server.shutdown" => self.server_shutdown(),
            _ => Err(RpcError::new(
                METHOD_NOT_FOUND,
                format!("unknown method {method}"),
            )),
        }
    }

    fn project_info(&self) -> MethodResult {
        let marker = project::discover(&self.root).ok().map(|p| p.marker);
        let methods: Vec<&str> = permission::METHODS.iter().map(|(n, _)| *n).collect();
        Ok(json!({
            "root": self.root.display().to_string(),
            "marker": marker,
            "server_version": env!("CARGO_PKG_VERSION"),
            "methods": methods,
            "permissions": {
                "read": true,
                "write": Permission::Write.granted(&self.config),
                "build": Permission::Build.granted(&self.config),
                "control": true,
            },
        }))
    }

    fn server_shutdown(&self) -> MethodResult {
        self.shutdown.store(true, Ordering::SeqCst);
        Ok(json!({ "stopping": true }))
    }

    /// Resolve a `.lean` path parameter inside the project root.
    fn lean_path_param(&self, params: &Value, key: &str) -> std::result::Result<PathBuf, RpcError> {
        let rel = str_param(params, key)?;
        let path = self.workspace.resolve(&rel).map_err(|e| {
            if e.code() == ErrorCode::PathOutsideWorkspace {
                RpcError::new(PATH_OUTSIDE_WORKSPACE, e.to_string())
            } else {
                RpcError::new(INVALID_PARAMS, e.to_string())
            }
        })?;
        if path.extension().and_then(|e| e.to_str()) != Some("lean") {
            return Err(RpcError::new(
                INVALID_PARAMS,
                format!("`{key}` must name a .lean file"),
            ));
        }
        Ok(path)
    }

    /// Resolve the project's index database, or fail if `index scan` has not run.
    fn open_index(&self) -> std::result::Result<tpt_lean_index::Index, RpcError> {
        let dirs = tpt_lean_core::dirs::ProjectDirs::new(&self.root);
        if !dirs.index_db().exists() {
            return Err(RpcError::new(
                APP_ERROR,
                "no index yet; run `tpt-lean index scan`",
            ));
        }
        tpt_lean_index::Index::open(&dirs.index_db()).map_err(app_error)
    }

    fn index_status(&self) -> MethodResult {
        let s = self.open_index()?.status().map_err(app_error)?;
        Ok(json!({
            "schema_version": s.schema_version,
            "files": s.files,
            "symbols": s.symbols,
            "packages": s.packages,
            "integrity_ok": s.integrity_ok,
        }))
    }

    /// Resolve a path parameter inside the workspace. Used by file-based methods.
    pub fn resolve_path(&self, rel: &str) -> std::result::Result<PathBuf, RpcError> {
        self.workspace.resolve(rel).map_err(app_error)
    }
}

pub(crate) fn app_error(e: Error) -> RpcError {
    RpcError::new(APP_ERROR, e.to_string())
}

pub(crate) fn str_param(params: &Value, key: &str) -> std::result::Result<String, RpcError> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| RpcError::new(INVALID_PARAMS, format!("missing string param `{key}`")))
}

/// Optional integer parameter clamped to `1..=max`, with a default.
pub(crate) fn limit_param(params: &Value, default: u64, max: u64) -> std::result::Result<usize, RpcError> {
    match params.get("limit") {
        None | Some(Value::Null) => Ok(default as usize),
        Some(v) => v
            .as_u64()
            .map(|n| n.clamp(1, max) as usize)
            .ok_or_else(|| RpcError::new(INVALID_PARAMS, "`limit` must be a positive integer")),
    }
}

/// Required 1-based integer parameter (line or column).
pub(crate) fn position_param(params: &Value, key: &str) -> std::result::Result<usize, RpcError> {
    params
        .get(key)
        .and_then(Value::as_u64)
        .filter(|n| *n >= 1 && *n <= u32::MAX as u64)
        .map(|n| n as usize)
        .ok_or_else(|| RpcError::new(INVALID_PARAMS, format!("`{key}` must be an integer >= 1")))
}

fn error_reply(id: Value, e: &RpcError) -> Value {
    let mut err = json!({ "code": e.code, "message": e.message });
    if let Some(data) = &e.data {
        err["data"] = data.clone();
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": err })
}

/// Serve one connection: one JSON-RPC request per line, one response per line.
/// Returns when the peer closes the connection or the server is asked to shut down.
pub fn serve_connection<S: Stream>(server: &Server, stream: S) -> io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone_stream()?);
    let mut writer = stream;
    let mut buf = String::new();
    loop {
        buf.clear();
        let n = reader
            .by_ref()
            .take(MAX_REQUEST_BYTES as u64 + 1)
            .read_line(&mut buf)?;
        if n == 0 {
            break;
        }
        if buf.len() > MAX_REQUEST_BYTES {
            let reply = error_reply(Value::Null, &RpcError::new(INVALID_REQUEST, "request too large"));
            writeln!(writer, "{reply}")?;
            break;
        }
        if buf.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&buf) {
            Ok(msg) => server.handle(&msg),
            Err(e) => Some(error_reply(
                Value::Null,
                &RpcError::new(PARSE_ERROR, e.to_string()),
            )),
        };
        if let Some(reply) = reply {
            writeln!(writer, "{reply}")?;
        }
        if server.is_shutdown_requested() {
            wake_listener(server);
            break;
        }
    }
    Ok(())
}

/// A connected byte stream that the server can read and write from two handles.
pub trait Stream: Read + Write + Send + Sized + 'static {
    fn try_clone_stream(&self) -> io::Result<Self>;
}

impl Stream for TcpStream {
    fn try_clone_stream(&self) -> io::Result<Self> {
        self.try_clone()
    }
}

#[cfg(unix)]
impl Stream for UnixStream {
    fn try_clone_stream(&self) -> io::Result<Self> {
        self.try_clone()
    }
}

/// Connect once to the listener so a blocked `accept` returns and sees the shutdown flag.
fn wake_listener(server: &Server) {
    let endpoint = lock(&server.listen_addr).clone();
    match endpoint {
        Some(Endpoint::Tcp(addr)) => {
            let target = if addr.ip().is_unspecified() {
                SocketAddr::from(([127, 0, 0, 1], addr.port()))
            } else {
                addr
            };
            let _ = TcpStream::connect_timeout(&target, Duration::from_secs(1));
        }
        #[cfg(unix)]
        Some(Endpoint::Unix(path)) => {
            let _ = UnixStream::connect(path);
        }
        None => {}
    }
}

/// Bind and serve on TCP until shutdown, one thread per connection. `port` 0 picks a free port.
pub fn serve(server: Server, port: u16) -> io::Result<()> {
    let listener = TcpListener::bind((server.bind_host(), port))?;
    serve_listener(Arc::new(server), listener)
}

/// Bind a Unix domain socket at `path` and serve until shutdown. A leftover socket file at
/// `path` is removed first; the new socket is made owner-only (0600).
#[cfg(unix)]
pub fn serve_unix(server: Server, path: &Path) -> io::Result<()> {
    let listener = bind_unix(path)?;
    serve_unix_listener(Arc::new(server), listener)
}

/// Bind a Unix socket at `path` with owner-only permissions.
#[cfg(unix)]
pub fn bind_unix(path: &Path) -> io::Result<UnixListener> {
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

/// Serve on an already-bound TCP listener until a `server.shutdown` request is handled.
pub fn serve_listener(server: Arc<Server>, listener: TcpListener) -> io::Result<()> {
    *lock(&server.listen_addr) = Some(Endpoint::Tcp(listener.local_addr()?));
    warm::spawn_reaper(Arc::clone(&server));
    for stream in listener.incoming() {
        if server.is_shutdown_requested() {
            break;
        }
        spawn_connection(&server, stream?);
    }
    server.release_warm_session();
    Ok(())
}

/// Serve on an already-bound Unix listener until a `server.shutdown` request is handled.
#[cfg(unix)]
pub fn serve_unix_listener(server: Arc<Server>, listener: UnixListener) -> io::Result<()> {
    let addr = listener.local_addr()?;
    let path = addr
        .as_pathname()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "unix listener has no path"))?
        .to_path_buf();
    *lock(&server.listen_addr) = Some(Endpoint::Unix(path));
    warm::spawn_reaper(Arc::clone(&server));
    for stream in listener.incoming() {
        if server.is_shutdown_requested() {
            break;
        }
        spawn_connection(&server, stream?);
    }
    server.release_warm_session();
    Ok(())
}

fn spawn_connection<S: Stream>(server: &Arc<Server>, stream: S) {
    let server = Arc::clone(server);
    thread::spawn(move || {
        let _ = serve_connection(&server, stream);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> Server {
        let tmp = std::env::temp_dir().join(format!("tpt-server-unit-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        Server::new(&tmp).unwrap()
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
    fn position_and_limit_params_are_validated() {
        assert_eq!(position_param(&json!({"line": 3}), "line").unwrap(), 3);
        assert!(position_param(&json!({"line": 0}), "line").is_err());
        assert!(position_param(&json!({"line": "3"}), "line").is_err());
        assert_eq!(limit_param(&json!({}), 10, 200).unwrap(), 10);
        assert_eq!(limit_param(&json!({"limit": 5000}), 10, 200).unwrap(), 200);
        assert!(limit_param(&json!({"limit": "x"}), 10, 200).is_err());
    }
}
