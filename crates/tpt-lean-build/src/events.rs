// SPDX-License-Identifier: MIT OR Apache-2.0
//! Structured build events (specification section 7.6) and event-derived reports.

use serde::{Deserialize, Serialize};

/// Where a cache hit was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheLayer {
    Memory,
    Disk,
    /// A shared directory (a team share or a CI cache), see `TPT_LEAN_SHARED_CACHE`.
    Shared,
}

/// One event in a build's stream. Serialized with the discriminant in `"event"`, for example
/// `{"event":"file_finished","run_id":"build_1","file":"Demo/Basic.lean","timestamp_ms":1,"duration_ms":542}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum BuildEvent {
    BuildStarted {
        run_id: String,
        timestamp_ms: u64,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        target: Option<String>,
    },
    CacheHit {
        run_id: String,
        timestamp_ms: u64,
        digest: String,
        layer: CacheLayer,
    },
    CacheMiss {
        run_id: String,
        timestamp_ms: u64,
        digest: String,
    },
    /// Emitted for a module before its `file_finished` or `file_failed`. Lake does not print a
    /// start line for successful modules, so the start time is derived from the reported duration.
    FileStarted {
        run_id: String,
        file: String,
        timestamp_ms: u64,
    },
    FileFinished {
        run_id: String,
        file: String,
        timestamp_ms: u64,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        duration_ms: Option<u64>,
    },
    FileFailed {
        run_id: String,
        file: String,
        timestamp_ms: u64,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        duration_ms: Option<u64>,
    },
    Diagnostic {
        run_id: String,
        timestamp_ms: u64,
        severity: String,
        file: String,
        line: u32,
        column: u32,
        message: String,
    },
    BuildFinished {
        run_id: String,
        timestamp_ms: u64,
        success: bool,
        duration_ms: u64,
        files_built: usize,
        files_failed: usize,
    },
}

impl BuildEvent {
    /// The run this event belongs to.
    pub fn run_id(&self) -> &str {
        match self {
            BuildEvent::BuildStarted { run_id, .. }
            | BuildEvent::CacheHit { run_id, .. }
            | BuildEvent::CacheMiss { run_id, .. }
            | BuildEvent::FileStarted { run_id, .. }
            | BuildEvent::FileFinished { run_id, .. }
            | BuildEvent::FileFailed { run_id, .. }
            | BuildEvent::Diagnostic { run_id, .. }
            | BuildEvent::BuildFinished { run_id, .. } => run_id,
        }
    }

    /// The stable event name, as serialized in `"event"`.
    pub fn name(&self) -> &'static str {
        match self {
            BuildEvent::BuildStarted { .. } => "build_started",
            BuildEvent::CacheHit { .. } => "cache_hit",
            BuildEvent::CacheMiss { .. } => "cache_miss",
            BuildEvent::FileStarted { .. } => "file_started",
            BuildEvent::FileFinished { .. } => "file_finished",
            BuildEvent::FileFailed { .. } => "file_failed",
            BuildEvent::Diagnostic { .. } => "diagnostic",
            BuildEvent::BuildFinished { .. } => "build_finished",
        }
    }
}

/// Result of one `build` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildOutcome {
    pub run_id: String,
    pub success: bool,
    pub timed_out: bool,
    pub exit_code: Option<i32>,
    pub digest: String,
    /// The key of the project state after Lake finished (equals `digest` unless Lake changed a
    /// key input, such as creating `lake-manifest.json`).
    pub post_digest: String,
    /// True when the key matched a successful earlier build recorded on disk.
    pub cache_hit: bool,
    pub files_built: usize,
    pub files_failed: usize,
    pub duration_ms: u64,
    /// Time spent computing the cache key (the cache-hit overhead).
    pub key_ms: u64,
}

/// One entry of an `--explain-slow` report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SlowFile {
    pub file: String,
    pub duration_ms: u64,
    pub failed: bool,
}

/// Rank the `n` slowest modules by the durations reported in `file_finished` and `file_failed`
/// events, slowest first. Ties keep the order in which the events arrived.
pub fn explain_slow(events: &[BuildEvent], n: usize) -> Vec<SlowFile> {
    let mut rows: Vec<SlowFile> = events
        .iter()
        .filter_map(|e| match e {
            BuildEvent::FileFinished {
                file,
                duration_ms: Some(d),
                ..
            } => Some(SlowFile {
                file: file.clone(),
                duration_ms: *d,
                failed: false,
            }),
            BuildEvent::FileFailed {
                file,
                duration_ms: Some(d),
                ..
            } => Some(SlowFile {
                file: file.clone(),
                duration_ms: *d,
                failed: true,
            }),
            _ => None,
        })
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.duration_ms));
    rows.truncate(n);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finished(file: &str, d: u64) -> BuildEvent {
        BuildEvent::FileFinished {
            run_id: "r".into(),
            file: file.into(),
            timestamp_ms: 0,
            duration_ms: Some(d),
        }
    }

    #[test]
    fn event_json_shape_matches_spec() {
        let e = BuildEvent::FileStarted {
            run_id: "build_123".into(),
            file: "Mathlib/Topology/Basic.lean".into(),
            timestamp_ms: 123456789,
        };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["event"], "file_started");
        assert_eq!(v["run_id"], "build_123");
        assert_eq!(v["file"], "Mathlib/Topology/Basic.lean");
        assert_eq!(v["timestamp_ms"], 123456789);
        assert!(v.get("duration_ms").is_none());
    }

    #[test]
    fn events_round_trip() {
        let e = finished("A.lean", 5);
        let s = serde_json::to_string(&e).unwrap();
        let back: BuildEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(back, e);
        assert_eq!(back.name(), "file_finished");
    }

    #[test]
    fn explain_slow_ranks_and_truncates() {
        let events = vec![
            finished("a", 10),
            finished("b", 300),
            BuildEvent::FileFailed {
                run_id: "r".into(),
                file: "c".into(),
                timestamp_ms: 0,
                duration_ms: Some(500),
            },
            finished("d", 20),
            BuildEvent::FileStarted {
                run_id: "r".into(),
                file: "e".into(),
                timestamp_ms: 0,
            },
        ];
        let top = explain_slow(&events, 2);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].file, "c");
        assert!(top[0].failed);
        assert_eq!(top[1].file, "b");
    }
}
