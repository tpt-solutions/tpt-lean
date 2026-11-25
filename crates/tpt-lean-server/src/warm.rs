// SPDX-License-Identifier: MIT OR Apache-2.0
//! A Lean language server kept running between requests.
//!
//! Starting `lake serve` and loading the project costs seconds per request, so the server keeps one
//! session open for the project. It is started on the first request that needs it, reused while
//! requests keep arriving, and stopped after [`WARM_IDLE_TIMEOUT`] without one, or when the server
//! shuts down. The CLI does not use it: each CLI command is its own process.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use tpt_lean_adapter::compat::Compatibility;
use tpt_lean_adapter::session::LspSession;
use tpt_lean_adapter::{discover_toolchain, lsp};
use tpt_lean_core::Result;

use crate::{lock, Server};

/// How long the session may sit unused before the Lean server is stopped.
pub const WARM_IDLE_TIMEOUT: Duration = Duration::from_secs(600);
/// How often the idle check runs.
const REAP_INTERVAL: Duration = Duration::from_secs(1);

/// The session and what it was started with.
pub(crate) struct WarmSession {
    pub(crate) session: LspSession,
    /// Lean version and compatibility, read once when the session started.
    pub(crate) version: (Option<String>, Compatibility),
    last_used: Instant,
}

/// The warm session slot of a server, and how many sessions it has started.
#[derive(Default)]
pub(crate) struct WarmState {
    pub(crate) session: Option<WarmSession>,
    pub(crate) starts: u32,
}

impl Server {
    /// Run `f` with the project's warm session, starting it if needed. The session is locked for
    /// the duration of `f`, so requests that use it are served one at a time.
    pub(crate) fn with_warm_session<T>(
        &self,
        f: impl FnOnce(&mut LspSession, &(Option<String>, Compatibility)) -> Result<T>,
    ) -> Result<T> {
        let mut state = lock(&self.warm);
        if state.session.is_none() {
            let timeout = Duration::from_secs(self.config().security.command_timeout_seconds);
            let tc = discover_toolchain();
            let (program, args) = lsp::server_command(self.root(), tc.lake.as_ref(), tc.lean.as_ref())?;
            let session = LspSession::new(&program, &args, self.root(), timeout);
            let version = tpt_lean_adapter::explain::installed_version(timeout);
            state.session = Some(WarmSession {
                session,
                version,
                last_used: Instant::now(),
            });
            state.starts += 1;
        }
        let warm = state.session.as_mut().expect("session was just started");
        warm.last_used = Instant::now();
        let result = f(&mut warm.session, &warm.version);
        warm.last_used = Instant::now();
        result
    }

    /// How many times a warm Lean session has been started. A server that reuses its session
    /// keeps this at 1 for a project.
    pub fn warm_session_starts(&self) -> u32 {
        lock(&self.warm).starts
    }

    /// Stop the warm session, if one is running. The next request starts a new one.
    pub fn release_warm_session(&self) {
        let taken = lock(&self.warm).session.take();
        if let Some(mut warm) = taken {
            warm.session.stop();
        }
    }

    /// Stop the warm session if it has been unused for `idle`. Returns whether it was stopped.
    pub fn reap_idle_session(&self, idle: Duration) -> bool {
        let stale = {
            let state = lock(&self.warm);
            matches!(&state.session, Some(w) if w.last_used.elapsed() >= idle)
        };
        if stale {
            self.release_warm_session();
        }
        stale
    }
}

/// Stop idle warm sessions in the background until the server shuts down.
pub(crate) fn spawn_reaper(server: Arc<Server>) {
    thread::spawn(move || {
        while !server.shutdown.load(Ordering::SeqCst) {
            thread::sleep(REAP_INTERVAL);
            server.reap_idle_session(WARM_IDLE_TIMEOUT);
        }
    });
}
