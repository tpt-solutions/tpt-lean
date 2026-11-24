// SPDX-License-Identifier: MIT OR Apache-2.0
//! A language-server session that survives crashes. It starts the server lazily, keeps the
//! text of every open document, and when the server dies it restarts it once and replays the
//! open documents. The failed request is then retried once; a second failure is reported.

use crate::lsp::{path_to_uri, server_command, LspClient};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tpt_lean_core::policy::CancelToken;
use tpt_lean_core::{Error, ErrorCode, Result};

/// Document kept so it can be replayed after a restart.
struct Doc {
    version: i64,
    text: String,
}

pub struct LspSession {
    program: PathBuf,
    args: Vec<String>,
    cwd: PathBuf,
    timeout: Duration,
    client: Option<LspClient>,
    /// Open documents keyed by URI.
    docs: BTreeMap<String, Doc>,
    restarts: u32,
}

impl LspSession {
    /// A session for an explicit server command. Nothing is started until the first request.
    pub fn new(program: &Path, args: &[String], cwd: &Path, timeout: Duration) -> Self {
        Self {
            program: program.to_path_buf(),
            args: args.to_vec(),
            cwd: cwd.to_path_buf(),
            timeout,
            client: None,
            docs: BTreeMap::new(),
            restarts: 0,
        }
    }

    /// A session for the project at `project_root`, using `lake serve` or `lean --server`.
    pub fn for_project(
        project_root: &Path,
        lake: Option<&PathBuf>,
        lean: Option<&PathBuf>,
        timeout: Duration,
    ) -> Result<Self> {
        let (program, args) = server_command(project_root, lake, lean)?;
        Ok(Self::new(&program, &args, project_root, timeout))
    }

    /// How many times the server has been restarted after a failure.
    pub fn restart_count(&self) -> u32 {
        self.restarts
    }

    /// Server version from the initialize result, once the server has started.
    pub fn server_version(&self) -> Option<&str> {
        self.client.as_ref().and_then(LspClient::server_version)
    }

    /// Kill the server process without forgetting documents. The next call restarts it.
    pub fn kill(&mut self) {
        if let Some(c) = self.client.as_mut() {
            c.kill();
        }
    }

    /// URIs of the documents currently open in this session.
    pub fn open_uris(&self) -> Vec<String> {
        self.docs.keys().cloned().collect()
    }

    /// Open the document on first use, otherwise send its new full text. Returns the URI.
    pub fn sync_document(&mut self, path: &Path, text: &str) -> Result<String> {
        let uri = path_to_uri(path);
        let next = match self.docs.get(&uri) {
            Some(d) => d.version + 1,
            None => 1,
        };
        let is_new = next == 1;
        self.with_recovery(|c| {
            if is_new {
                c.open_with_version(&uri, next, text)
            } else {
                c.change_with_version(&uri, next, text)
            }
        })?;
        self.docs.insert(
            uri.clone(),
            Doc {
                version: next,
                text: text.to_string(),
            },
        );
        Ok(uri)
    }

    /// Close a document; it is no longer replayed after a restart.
    pub fn close_document(&mut self, path: &Path) -> Result<()> {
        let uri = path_to_uri(path);
        self.docs.remove(&uri);
        match self.client.as_mut() {
            Some(c) => c.close_document(path),
            None => Ok(()),
        }
    }

    /// Send a request with the session timeout, recovering from a crashed server once.
    pub fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let timeout = self.timeout;
        self.with_recovery(|c| c.request(method, params.clone(), timeout))
    }

    /// Like [`LspSession::request`] but abandons the request once `cancel` is set.
    pub fn request_cancellable(
        &mut self,
        method: &str,
        params: Value,
        cancel: &CancelToken,
    ) -> Result<Value> {
        let timeout = self.timeout;
        self.with_recovery(|c| c.request_cancellable(method, params.clone(), timeout, cancel))
    }

    /// Diagnostics the server has published for `uri` so far (empty when none).
    pub fn diagnostics(&mut self, uri: &str) -> Vec<Value> {
        self.published_diagnostics(uri).unwrap_or_default()
    }

    /// Latest diagnostics published for `uri`, or `None` when the server has not published yet.
    pub fn published_diagnostics(&mut self, uri: &str) -> Option<Vec<Value>> {
        self.client.as_mut().and_then(|c| c.published_diagnostics(uri))
    }

    /// Stop the server cleanly. The session can still be used; the next call starts a new server.
    pub fn stop(&mut self) {
        if let Some(c) = self.client.take() {
            c.shutdown(Duration::from_secs(10));
        }
    }

    /// Stop the server cleanly and consume the session.
    pub fn shutdown(mut self) {
        self.stop();
    }

    /// Run `f` against a live client. If the server is dead or the call fails for a reason
    /// other than cancellation or timeout, restart once and retry the call once.
    fn with_recovery<T>(&mut self, mut f: impl FnMut(&mut LspClient) -> Result<T>) -> Result<T> {
        self.ensure_running()?;
        let first = match self.client.as_mut() {
            Some(c) => f(c),
            None => Err(Error::new(ErrorCode::Internal, "language server not running")),
        };
        let err = match first {
            Ok(v) => return Ok(v),
            Err(e) => e,
        };
        let alive = self.client.as_mut().is_some_and(|c| c.is_alive());
        let crashed = !alive || err.code() == ErrorCode::Io;
        if crashed_retry_disallowed(&err) || !crashed {
            return Err(err);
        }
        if let Err(restart_err) = self.restart() {
            return Err(Error::new(
                ErrorCode::Io,
                format!(
                    "language server failed ({}); restart failed: {}",
                    err.message(),
                    restart_err.message()
                ),
            ));
        }
        match self.client.as_mut() {
            Some(c) => f(c),
            None => Err(err),
        }
    }

    /// Start the server if it was never started, or restart it if it has exited.
    fn ensure_running(&mut self) -> Result<()> {
        let alive = self.client.as_mut().map(|c| c.is_alive());
        match alive {
            Some(true) => Ok(()),
            Some(false) => self.restart(),
            None => self.start_client(),
        }
    }

    fn restart(&mut self) -> Result<()> {
        if let Some(mut old) = self.client.take() {
            old.kill();
        }
        self.restarts += 1;
        self.start_client()
    }

    fn start_client(&mut self) -> Result<()> {
        let mut client = LspClient::start(&self.program, &self.args, &self.cwd, self.timeout)?;
        for (uri, doc) in &self.docs {
            client.open_with_version(uri, doc.version, &doc.text)?;
        }
        self.client = Some(client);
        Ok(())
    }
}

/// Errors that must not trigger a restart: the caller gave up, or the server was slow.
fn crashed_retry_disallowed(err: &Error) -> bool {
    crate::lsp::is_cancelled(err) || err.code() == ErrorCode::Timeout
}
