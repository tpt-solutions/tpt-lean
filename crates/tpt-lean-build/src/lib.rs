// SPDX-License-Identifier: MIT OR Apache-2.0
//! Build orchestration for Lake projects: calls `lake build` through the adapter (no shell),
//! turns its progress output into a structured event stream, computes cache keys, and records
//! successful builds in a disk cache.
//!
//! Lake remains the authority on what to rebuild. The cache reports hits and misses and lets
//! watch mode skip rebuilds whose inputs are unchanged; it never serves outputs on its own.
//!
//! Not implemented: parallel orchestration beyond Lake's own scheduler (see docs/build.md,
//! Parallel orchestration), shared team and CI caches, and deep build graphs.

pub mod cache;
pub mod events;
pub mod key;
pub mod progress;
pub mod project;
pub mod schedule;
pub mod watch;

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use std::sync::Arc;
use tpt_lean_adapter::{discover_toolchain, execute_streaming, lean_policy, Command, Diagnostic, LineSink};
use tpt_lean_core::{Error, ErrorCode};
use tpt_lean_core::policy::CancelToken;
use tpt_lean_core::Result;

pub use cache::{
    prune_older_than, records_dir, status, BuildRecord, CacheStatus, DiskCache, MemoryCache, PruneReport,
};
pub use events::{explain_slow, BuildEvent, BuildOutcome, CacheLayer, SlowFile};
pub use key::{cache_key_for, key_from_parts, probe_tools, Tools};
pub use project::{fingerprint, scan_project, Fingerprint, ProjectScan};
pub use schedule::{plan, Plan};
pub use watch::watch;

static RUN_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub(crate) fn new_run_id() -> String {
    let n = RUN_COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("build_{}_{n}", now_ms())
}

/// Run `lake build [target]` in `project_root` and report it as a stream of [`BuildEvent`]s.
///
/// Events are delivered to `sink` in order: `build_started`, `cache_hit` or `cache_miss`, one or
/// more `file_started` / `file_finished` / `file_failed` groups, `diagnostic`s, and finally
/// `build_finished`. Lake's output is read line by line while it runs, so file events arrive as Lake
/// reports them. A successful build is recorded in the disk cache.
///
/// `timeout_secs` bounds the Lake process; on timeout the outcome has `timed_out: true`.
pub fn build(
    project_root: &Path,
    target: Option<&str>,
    timeout_secs: u64,
    sink: &mut dyn FnMut(&BuildEvent),
) -> Result<BuildOutcome> {
    build_cancellable(project_root, target, timeout_secs, &CancelToken::new(), sink)
}

/// Like [`build`], but the Lake process is stopped when `cancel` is cancelled. A cancelled build
/// is not recorded in the cache.
pub fn build_cancellable(
    project_root: &Path,
    target: Option<&str>,
    timeout_secs: u64,
    cancel: &CancelToken,
    sink: &mut dyn FnMut(&BuildEvent),
) -> Result<BuildOutcome> {
    build_cancellable_with_run_id(new_run_id(), project_root, target, timeout_secs, cancel, sink)
}

/// Like [`build_cancellable`], but every event and the outcome carry `run_id`, so a caller that
/// has already named the build (the server does) sees the same id in both.
pub fn build_cancellable_with_run_id(
    run_id: String,
    project_root: &Path,
    target: Option<&str>,
    timeout_secs: u64,
    cancel: &CancelToken,
    sink: &mut dyn FnMut(&BuildEvent),
) -> Result<BuildOutcome> {
    let t0 = now_ms();

    let tc = discover_toolchain();
    let key_start = Instant::now();
    let scan = scan_project(project_root)?;
    let tools = probe_tools(&tc)?;
    let key = key::key_from_parts(&scan, &tools, target);
    let digest = key.digest();
    let key_ms = key_start.elapsed().as_millis() as u64;

    let root_str = project_root.display().to_string();
    let disk = DiskCache::default_location();
    let hit = match &disk {
        Some(c) => c.lookup(&digest, &root_str).ok().flatten().is_some(),
        None => false,
    };

    sink(&BuildEvent::BuildStarted {
        run_id: run_id.clone(),
        timestamp_ms: t0,
        target: target.map(str::to_string),
    });
    if hit {
        sink(&BuildEvent::CacheHit {
            run_id: run_id.clone(),
            timestamp_ms: t0,
            digest: digest.clone(),
            layer: CacheLayer::Disk,
        });
    } else {
        sink(&BuildEvent::CacheMiss {
            run_id: run_id.clone(),
            timestamp_ms: t0,
            digest: digest.clone(),
        });
    }

    let lake = Command::LakeBuild {
        target: target.map(str::to_string),
    };
    let policy = lean_policy(timeout_secs);
    // Lake's output is read on another thread and sent here line by line, so progress events reach
    // the sink while the build runs. The worker's result is collected once the lines are drained.
    let (tx, rx) = std::sync::mpsc::channel::<progress::Progress>();
    let line_sink: LineSink = Arc::new(move |line: &str| {
        if let Some(p) = progress::parse_line(line) {
            let _ = tx.send(p);
        }
    });
    let mut built = 0usize;
    let mut failed = 0usize;
    let result = std::thread::scope(|scope| {
        let worker = scope.spawn(|| execute_streaming(&tc, &lake, Some(project_root), &policy, cancel, Some(line_sink)));
        for p in rx {
            let t = now_ms();
            match p {
                progress::Progress::Started { module } => {
                    sink(&BuildEvent::FileStarted {
                        run_id: run_id.clone(),
                        file: display_file(&scan, &module),
                        timestamp_ms: t,
                    });
                }
                progress::Progress::Finished { module, duration_ms } => {
                    built += 1;
                    let file = display_file(&scan, &module);
                    emit_started_from_duration(sink, &run_id, &file, t0, t, duration_ms);
                    sink(&BuildEvent::FileFinished {
                        run_id: run_id.clone(),
                        file,
                        timestamp_ms: t,
                        duration_ms,
                    });
                }
                progress::Progress::Failed { module, duration_ms } => {
                    failed += 1;
                    let file = display_file(&scan, &module);
                    emit_started_from_duration(sink, &run_id, &file, t0, t, duration_ms);
                    sink(&BuildEvent::FileFailed {
                        run_id: run_id.clone(),
                        file,
                        timestamp_ms: t,
                        duration_ms,
                    });
                }
                progress::Progress::Artifact { .. } | progress::Progress::Summary { .. } => {}
            }
        }
        worker
            .join()
            .unwrap_or_else(|_| Err(Error::new(ErrorCode::Internal, "the Lake worker thread panicked")))
    });
    let t1 = now_ms();
    let res = match result {
        Ok(r) => r,
        Err(e) => {
            sink(&finished_event(&run_id, t1, false, 0, 0));
            return Err(e);
        }
    };

    for d in &res.diagnostics {
        sink(&diagnostic_event(&run_id, t1, d));
    }

    let success = res.output.success();
    // Lake may write files the key covers (for example `lake-manifest.json` on the first build),
    // so the state left behind can have a different key. Both states are recorded on success.
    let post_digest = match scan_project(project_root) {
        Ok(post) => key::key_from_parts(&post, &tools, target).digest(),
        Err(_) => digest.clone(),
    };
    if success {
        if let Some(cache) = &disk {
            let mut digests = vec![digest.clone()];
            if post_digest != digest {
                digests.push(post_digest.clone());
            }
            for d in digests {
                let record = BuildRecord {
                    digest: d,
                    project_root: root_str.clone(),
                    finished_at_ms: t1,
                    duration_ms: t1.saturating_sub(t0),
                    files_built: built,
                };
                if let Err(e) = cache.store(&record) {
                    sink(&BuildEvent::Diagnostic {
                        run_id: run_id.clone(),
                        timestamp_ms: t1,
                        severity: "warning".into(),
                        file: String::new(),
                        line: 0,
                        column: 0,
                        message: format!("could not record build in cache: {}", e.message()),
                    });
                }
            }
        }
    }

    let duration_ms = t1.saturating_sub(t0);
    sink(&BuildEvent::BuildFinished {
        run_id: run_id.clone(),
        timestamp_ms: t1,
        success,
        duration_ms,
        files_built: built,
        files_failed: failed,
    });

    Ok(BuildOutcome {
        run_id,
        success,
        timed_out: res.output.timed_out,
        exit_code: res.output.exit_code,
        digest,
        post_digest,
        cache_hit: hit,
        files_built: built,
        files_failed: failed,
        duration_ms,
        key_ms,
    })
}

/// Lake reports a module's duration when it finishes. Emit `file_started` at the derived start
/// time (finish minus duration, never before the build started).
fn emit_started_from_duration(
    sink: &mut dyn FnMut(&BuildEvent),
    run_id: &str,
    file: &str,
    t0: u64,
    t1: u64,
    duration_ms: Option<u64>,
) {
    if let Some(d) = duration_ms {
        sink(&BuildEvent::FileStarted {
            run_id: run_id.to_string(),
            file: file.to_string(),
            timestamp_ms: t1.saturating_sub(d).max(t0),
        });
    }
}

fn display_file(scan: &ProjectScan, module: &str) -> String {
    match scan.file_for_module(module) {
        Some(f) => f.rel_path.clone(),
        None => module.to_string(),
    }
}

fn finished_event(run_id: &str, ts: u64, success: bool, built: usize, failed: usize) -> BuildEvent {
    BuildEvent::BuildFinished {
        run_id: run_id.to_string(),
        timestamp_ms: ts,
        success,
        duration_ms: 0,
        files_built: built,
        files_failed: failed,
    }
}

fn diagnostic_event(run_id: &str, ts: u64, d: &Diagnostic) -> BuildEvent {
    let severity = serde_json::to_value(d.severity)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "error".into());
    BuildEvent::Diagnostic {
        run_id: run_id.to_string(),
        timestamp_ms: ts,
        severity,
        file: d.file.clone(),
        line: d.line,
        column: d.column,
        message: d.message.clone(),
    }
}

/// Build-cache state for a project, computed without running Lake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheProbe {
    /// Digest of the project's build key: sources, imports, toolchain, flags and target.
    pub digest: String,
    /// Whether a successful build with this digest is recorded in the disk cache.
    pub hit: bool,
    /// Whether a cache directory is available for this user.
    pub available: bool,
    /// Build records stored in the cache, across all projects.
    pub entries: u64,
}

/// Look up whether a successful build with the project's current key is recorded. Never runs
/// Lake and never writes to the cache.
pub fn probe_cache(
    project_root: &Path,
    target: Option<&str>,
    tc: &tpt_lean_adapter::Toolchain,
) -> Result<CacheProbe> {
    let scan = scan_project(project_root)?;
    let tools = probe_tools(tc)?;
    let digest = key::key_from_parts(&scan, &tools, target).digest();
    let Some(disk) = cache::DiskCache::default_location() else {
        return Ok(CacheProbe {
            digest,
            hit: false,
            available: false,
            entries: 0,
        });
    };
    let hit = disk
        .lookup(&digest, &project_root.display().to_string())?
        .is_some();
    let entries = cache::status(disk.root()).files;
    Ok(CacheProbe {
        digest,
        hit,
        available: true,
        entries,
    })
}

/// Record a successful build under `digest`, the same record [`build`] writes when Lake passes.
/// Does nothing when no cache directory is available.
pub fn record_success(project_root: &Path, digest: &str, files_built: usize, duration_ms: u64) -> Result<()> {
    let Some(disk) = cache::DiskCache::default_location() else {
        return Ok(());
    };
    disk.store(&cache::BuildRecord {
        digest: digest.to_string(),
        project_root: project_root.display().to_string(),
        finished_at_ms: now_ms(),
        duration_ms,
        files_built,
    })
}
