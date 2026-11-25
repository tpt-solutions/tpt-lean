// SPDX-License-Identifier: MIT OR Apache-2.0
//! Build methods. `build.run` starts `lake build` on a background thread and returns at once;
//! `build.status` reports the latest build, or one by id; `build.cancel` stops a running build.
//! One build runs at a time per server, so a second `build.run` while one runs is refused.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tpt_lean_adapter::validate_target;
use tpt_lean_core::policy::CancelToken;

use crate::{app_error, lock, MethodResult, RpcError, Server, APP_ERROR, INVALID_PARAMS};

/// Builds kept for `build.status`. Older finished builds are dropped.
const MAX_KEPT: usize = 20;

static NEXT_BUILD: AtomicU64 = AtomicU64::new(1);

/// Where a build is. A finished build keeps its summary as JSON.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum BuildState {
    Running,
    Finished(Value),
}

/// One build started by this server. The thread running it and the request handlers share it.
#[derive(Debug, Clone)]
pub(crate) struct BuildJob {
    pub(crate) id: String,
    pub(crate) cancel: CancelToken,
    pub(crate) state: Arc<Mutex<BuildState>>,
}

impl BuildJob {
    fn is_running(&self) -> bool {
        matches!(*lock(&self.state), BuildState::Running)
    }
}

/// The builds this server has started, oldest first.
#[derive(Debug, Default)]
pub(crate) struct Builds {
    jobs: Vec<BuildJob>,
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

impl Server {
    /// Start `lake build` in the background. The result is the run id; poll `build.status`.
    pub(crate) fn build_run(&self, params: &Value) -> MethodResult {
        let target = match params.get("target") {
            None | Some(Value::Null) => None,
            Some(v) => {
                let t = v
                    .as_str()
                    .ok_or_else(|| RpcError::new(INVALID_PARAMS, "`target` must be a string"))?;
                Some(validate_target(t).map_err(app_error)?.to_string())
            }
        };

        let mut builds = lock(&self.builds);
        if let Some(running) = builds.jobs.iter().rev().find(|j| j.is_running()) {
            return Err(RpcError::new(
                APP_ERROR,
                format!(
                    "build {} is already running; cancel it or wait for it",
                    running.id
                ),
            ));
        }
        let id = format!("build_{}_{}", now_ms(), NEXT_BUILD.fetch_add(1, Ordering::SeqCst));
        let job = BuildJob {
            id: id.clone(),
            cancel: CancelToken::new(),
            state: Arc::new(Mutex::new(BuildState::Running)),
        };
        builds.jobs.push(job.clone());
        while builds.jobs.len() > MAX_KEPT {
            // Only finished builds are dropped; the running one is always the newest.
            builds.jobs.remove(0);
        }
        drop(builds);

        let root = self.root().to_path_buf();
        let timeout = self.config().security.command_timeout_seconds;
        let thread_job = job.clone();
        std::thread::spawn(move || {
            let outcome = tpt_lean_build::build_cancellable(
                &root,
                target.as_deref(),
                timeout,
                &thread_job.cancel,
                &mut |_| {},
            );
            let cancelled = thread_job.cancel.is_cancelled();
            let summary = match outcome {
                Ok(o) => json!({
                    "run_id": thread_job.id,
                    "success": o.success,
                    "timed_out": o.timed_out,
                    "exit_code": o.exit_code,
                    "digest": o.digest,
                    "post_digest": o.post_digest,
                    "cache_hit": o.cache_hit,
                    "files_built": o.files_built,
                    "files_failed": o.files_failed,
                    "duration_ms": o.duration_ms,
                    "cancelled": cancelled,
                    "error": Value::Null,
                }),
                Err(e) => json!({
                    "run_id": thread_job.id,
                    "success": false,
                    "timed_out": false,
                    "exit_code": Value::Null,
                    "cancelled": cancelled,
                    "error": e.to_string(),
                }),
            };
            *lock(&thread_job.state) = BuildState::Finished(summary);
        });

        Ok(json!({ "run_id": id, "status": "running" }))
    }

    /// The latest build, or the build with `run_id`. `status` is `none` when no build was started.
    pub(crate) fn build_status(&self, params: &Value) -> MethodResult {
        let job =
            {
                let builds = lock(&self.builds);
                match params.get("run_id").and_then(Value::as_str) {
                    Some(id) => builds.jobs.iter().find(|j| j.id == id).cloned().ok_or_else(|| {
                        RpcError::new(INVALID_PARAMS, format!("no build with run_id {id:?}"))
                    })?,
                    None => match builds.jobs.last().cloned() {
                        Some(job) => job,
                        None => {
                            return Ok(json!({ "run_id": null, "status": "none", "result": null }));
                        }
                    },
                }
            };
        let state = lock(&job.state).clone();
        Ok(match state {
            BuildState::Running => json!({ "run_id": job.id, "status": "running", "result": null }),
            BuildState::Finished(summary) => {
                let status = if summary["cancelled"] == true {
                    "cancelled"
                } else if summary["success"] == true {
                    "passed"
                } else {
                    "failed"
                };
                json!({ "run_id": job.id, "status": status, "result": summary })
            }
        })
    }

    /// Stop a running build: the one with `run_id`, or the newest running one. Reports whether
    /// a running build was asked to stop; the build itself finishes as `cancelled` shortly after.
    pub(crate) fn build_cancel(&self, params: &Value) -> MethodResult {
        let builds = lock(&self.builds);
        let job = match params.get("run_id").and_then(Value::as_str) {
            Some(id) => match builds.jobs.iter().find(|j| j.id == id) {
                Some(job) => job.clone(),
                None => {
                    return Err(RpcError::new(
                        INVALID_PARAMS,
                        format!("no build with run_id {id:?}"),
                    ))
                }
            },
            None => match builds.jobs.iter().rev().find(|j| j.is_running()) {
                Some(job) => job.clone(),
                None => return Ok(json!({ "run_id": null, "cancelled": false })),
            },
        };
        drop(builds);
        if job.is_running() {
            job.cancel.cancel();
            Ok(json!({ "run_id": job.id, "cancelled": true }))
        } else {
            Ok(json!({ "run_id": job.id, "cancelled": false, "status": "finished" }))
        }
    }
}
