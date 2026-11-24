// SPDX-License-Identifier: MIT OR Apache-2.0
//! Server lifecycle: `.tpt-lean/server.json` records the running server's pid and port. `status`
//! and `stop` read it and talk to the server over its TCP transport.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tpt_lean_core::{Error, ErrorCode, Result};

use crate::client::{call, is_ok_response};
use crate::{serve_listener, Server};

/// Directory (relative to the project root) that holds the state file.
pub const DATA_DIR: &str = ".tpt-lean";
/// State file name inside [`DATA_DIR`].
pub const STATE_FILE: &str = "server.json";

const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const STOP_WAIT: Duration = Duration::from_secs(10);

/// Contents of `.tpt-lean/server.json` while a server is running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerState {
    pub pid: u32,
    pub port: u16,
    pub project: String,
    pub version: String,
}

impl ServerState {
    pub fn to_json(&self) -> Value {
        json!({
            "pid": self.pid,
            "port": self.port,
            "project": self.project,
            "version": self.version,
        })
    }

    fn from_json(v: &Value) -> Option<Self> {
        Some(Self {
            pid: u32::try_from(v.get("pid")?.as_u64()?).ok()?,
            port: u16::try_from(v.get("port")?.as_u64()?).ok()?,
            project: v.get("project")?.as_str()?.to_string(),
            version: v.get("version")?.as_str()?.to_string(),
        })
    }
}

pub fn state_path(root: &Path) -> PathBuf {
    root.join(DATA_DIR).join(STATE_FILE)
}

pub fn read_state(root: &Path) -> Result<Option<ServerState>> {
    let path = state_path(root);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::from(e).with_path(&path)),
    };
    let value: Value = serde_json::from_str(&text).map_err(|e| {
        Error::new(ErrorCode::ConfigInvalid, format!("corrupt server state: {e}")).with_path(&path)
    })?;
    ServerState::from_json(&value).map(Some).ok_or_else(|| {
        Error::new(ErrorCode::ConfigInvalid, "server state has missing fields").with_path(&path)
    })
}

fn write_state(root: &Path, state: &ServerState) -> Result<()> {
    let path = state_path(root);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Error::from(e).with_path(dir))?;
    }
    std::fs::write(&path, state.to_json().to_string()).map_err(|e| Error::from(e).with_path(&path))
}

fn remove_state(root: &Path) -> Result<()> {
    let path = state_path(root);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::from(e).with_path(&path)),
    }
}

/// Run the server in the foreground for `root` until a `server.shutdown` request arrives.
/// `port` 0 picks a free port. Writes the state file after binding and removes it on exit.
pub fn run_foreground(root: &Path, port: u16) -> Result<()> {
    let server = Server::new(root)?;
    let project = server.root().to_path_buf();
    if let Some(existing) = read_state(&project)? {
        if is_alive(existing.port) {
            return Err(Error::new(
                ErrorCode::InvalidArgument,
                format!(
                    "a server is already running for this project (pid {}, port {})",
                    existing.pid, existing.port
                ),
            ));
        }
    }
    let listener = TcpListener::bind((server.bind_host(), port))?;
    let state = ServerState {
        pid: std::process::id(),
        port: listener.local_addr()?.port(),
        project: project.display().to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    };
    write_state(&project, &state)?;
    let result = serve_listener(Arc::new(server), listener);
    let cleanup = remove_state(&project);
    result?;
    cleanup
}

fn is_alive(port: u16) -> bool {
    matches!(call(port, "project.info", json!({}), PROBE_TIMEOUT), Ok(r) if is_ok_response(&r))
}

/// Whether a server is running for `root`, and its recorded pid and port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusReport {
    pub running: bool,
    /// A state file exists but the server does not answer.
    pub stale: bool,
    pub state: Option<ServerState>,
}

impl StatusReport {
    pub fn to_json(&self) -> Value {
        json!({
            "running": self.running,
            "stale": self.stale,
            "pid": self.state.as_ref().map(|s| s.pid),
            "port": self.state.as_ref().map(|s| s.port),
            "project": self.state.as_ref().map(|s| s.project.clone()),
        })
    }
}

/// Report whether a server is running for `root`. Probes the server with `project.info`.
pub fn status(root: &Path) -> Result<StatusReport> {
    match read_state(root)? {
        None => Ok(StatusReport {
            running: false,
            stale: false,
            state: None,
        }),
        Some(state) => {
            let alive = is_alive(state.port);
            Ok(StatusReport {
                running: alive,
                stale: !alive,
                state: Some(state),
            })
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopOutcome {
    /// No state file: nothing to stop.
    NotRunning,
    /// The server acknowledged shutdown and removed its state file.
    Stopped,
    /// The state file pointed at a server that did not answer; the file was removed.
    StaleRemoved,
    /// The server acknowledged shutdown but had not exited within the wait limit.
    TimedOut,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopReport {
    pub outcome: StopOutcome,
    pub pid: Option<u32>,
}

impl StopReport {
    pub fn to_json(&self) -> Value {
        let outcome = match self.outcome {
            StopOutcome::NotRunning => "not_running",
            StopOutcome::Stopped => "stopped",
            StopOutcome::StaleRemoved => "stale_removed",
            StopOutcome::TimedOut => "timed_out",
        };
        json!({ "outcome": outcome, "pid": self.pid })
    }
}

/// Ask the server for `root` to shut down with `server.shutdown`, then wait for it to exit.
pub fn stop(root: &Path) -> Result<StopReport> {
    let Some(state) = read_state(root)? else {
        return Ok(StopReport {
            outcome: StopOutcome::NotRunning,
            pid: None,
        });
    };
    let pid = Some(state.pid);
    match call(state.port, "server.shutdown", json!({}), PROBE_TIMEOUT) {
        Ok(r) if is_ok_response(&r) => {
            let deadline = Instant::now() + STOP_WAIT;
            while state_path(root).exists() {
                if Instant::now() >= deadline {
                    return Ok(StopReport {
                        outcome: StopOutcome::TimedOut,
                        pid,
                    });
                }
                thread::sleep(Duration::from_millis(50));
            }
            Ok(StopReport {
                outcome: StopOutcome::Stopped,
                pid,
            })
        }
        _ => {
            remove_state(root)?;
            Ok(StopReport {
                outcome: StopOutcome::StaleRemoved,
                pid,
            })
        }
    }
}
