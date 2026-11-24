// SPDX-License-Identifier: MIT OR Apache-2.0
//! Watch mode: poll the project and rebuild when its inputs change.
//!
//! Polling uses only `std`: every `poll_ms` the content hash of each input file is compared with
//! the previous poll. When anything differs, the cache key is recomputed, and the build is skipped
//! if that key already built successfully in this process (memory layer). A save that leaves
//! contents unchanged therefore does not invoke Lake. No file watcher crate is used.

use crate::cache::{BuildRecord, MemoryCache};
use crate::events::{BuildEvent, CacheLayer};
use crate::project::fingerprint;
use crate::{build, cache_key_for, new_run_id, now_ms};
use std::path::Path;
use std::thread;
use std::time::Duration;
use tpt_lean_core::policy::CancelToken;
use tpt_lean_core::Result;

/// Granularity at which the sleep between polls checks for cancellation.
const SLICE_MS: u64 = 50;

/// Build once, then rebuild whenever the project changes, until `cancel` is set.
///
/// Every build reports through `sink` as in [`build`]. A skipped rebuild emits `build_started`,
/// a memory-layer `cache_hit`, and `build_finished` with `files_built: 0`. Errors from later
/// rebuilds are reported through `build_finished` and do not stop the watch; errors from the
/// initial build are returned.
pub fn watch(
    project_root: &Path,
    target: Option<&str>,
    timeout_secs: u64,
    poll_ms: u64,
    cancel: &CancelToken,
    sink: &mut dyn FnMut(&BuildEvent),
) -> Result<()> {
    let mut memory = MemoryCache::new();
    let root_str = project_root.display().to_string();

    // Fingerprint before the first build, so an edit made while Lake runs is seen by the loop.
    let mut last = fingerprint(project_root)?;
    let first = build(project_root, target, timeout_secs, sink)?;
    remember(&mut memory, &first, &root_str);

    let poll = poll_ms.max(SLICE_MS);
    while !cancel.is_cancelled() {
        sleep_cancellable(poll, cancel);
        if cancel.is_cancelled() {
            break;
        }
        let now = match fingerprint(project_root) {
            Ok(f) => f,
            Err(_) => continue,
        };
        if now == last {
            continue;
        }
        last = now;

        let digest = match cache_key_for(project_root, target) {
            Ok(k) => k.digest(),
            Err(_) => continue,
        };
        if let Some(rec) = memory.get(&digest).cloned() {
            let run_id = new_run_id();
            let ts = now_ms();
            sink(&BuildEvent::BuildStarted {
                run_id: run_id.clone(),
                timestamp_ms: ts,
                target: target.map(str::to_string),
            });
            sink(&BuildEvent::CacheHit {
                run_id: run_id.clone(),
                timestamp_ms: ts,
                digest: rec.digest.clone(),
                layer: CacheLayer::Memory,
            });
            sink(&BuildEvent::BuildFinished {
                run_id,
                timestamp_ms: ts,
                success: true,
                duration_ms: 0,
                files_built: 0,
                files_failed: 0,
            });
            continue;
        }
        if let Ok(outcome) = build(project_root, target, timeout_secs, sink) {
            remember(&mut memory, &outcome, &root_str);
        }
    }
    Ok(())
}

fn remember(memory: &mut MemoryCache, outcome: &crate::BuildOutcome, root: &str) {
    if outcome.success {
        let mut digests = vec![outcome.digest.clone()];
        if outcome.post_digest != outcome.digest {
            digests.push(outcome.post_digest.clone());
        }
        for d in digests {
            memory.put(BuildRecord {
                digest: d,
                project_root: root.to_string(),
                finished_at_ms: now_ms(),
                duration_ms: outcome.duration_ms,
                files_built: outcome.files_built,
            });
        }
    }
}

fn sleep_cancellable(total_ms: u64, cancel: &CancelToken) {
    let mut left = total_ms;
    while left > 0 && !cancel.is_cancelled() {
        let step = left.min(SLICE_MS);
        thread::sleep(Duration::from_millis(step));
        left -= step;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let p = std::env::temp_dir().join(format!(
                "tpt-lean-build-watch-{tag}-{}-{nanos}",
                std::process::id()
            ));
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn fingerprint_detects_edits_and_ignores_build_outputs() {
        let tmp = TempDir::new("fp");
        let src = tmp.0.join("Main.lean");
        std::fs::write(&src, "def a := 1\n").unwrap();
        let before = fingerprint(&tmp.0).unwrap();
        assert_eq!(before, fingerprint(&tmp.0).unwrap());

        std::fs::write(&src, "def a := 2\n").unwrap();
        assert_ne!(before, fingerprint(&tmp.0).unwrap());

        // Lake writes outputs under `.lake`; those must not change the fingerprint.
        let outputs = tmp.0.join(".lake").join("build");
        std::fs::create_dir_all(&outputs).unwrap();
        let mid = fingerprint(&tmp.0).unwrap();
        std::fs::write(outputs.join("Main.olean"), "bin").unwrap();
        assert_eq!(mid, fingerprint(&tmp.0).unwrap());
    }

    #[test]
    fn cancelled_token_stops_sleep_quickly() {
        let t = CancelToken::new();
        t.cancel();
        let start = std::time::Instant::now();
        sleep_cancellable(10_000, &t);
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
