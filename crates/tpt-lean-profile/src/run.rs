// SPDX-License-Identifier: MIT OR Apache-2.0
//! Running Lean on an instrumented copy of a file and assembling the report.
//!
//! The profiler options are placed in a temporary copy of the file, because the adapter's typed
//! `lean` command takes only a file path. The copy is removed afterwards. Lean is run with the
//! project root as its working directory, but Lean does not search the working directory for
//! modules: imports resolve through the toolchain and `LEAN_PATH`, as with a plain `lean` run.

use crate::analysis::{analyze, round_ms};
use crate::report::{suggest, ProfileReport, FORMAT_VERSION};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tpt_lean_adapter::{discover_toolchain, execute, Command, Severity};
use tpt_lean_core::policy::{CancelToken, ProcessPolicy};
use tpt_lean_core::{Error, ErrorCode, Result};

/// Captured stdout plus stderr allowed for one profiled run. Trace output is large.
const OUTPUT_LIMIT_BYTES: usize = 256 * 1024 * 1024;
/// Declarations whose trace node is shorter than this are omitted from the trace.
const TRACE_THRESHOLD_MS: u32 = 2;
/// Plain profiler lines shorter than this are omitted.
const PROFILER_THRESHOLD_MS: u32 = 1;

const NOT_MEASURED: &[&str] = &[
    "memory usage: not measured (Lean does not report it per declaration, and no portable limit is implemented)",
    "flamegraph output: not implemented",
    "slow import detection and dependency bottleneck graph: not implemented",
    "build-step timing and cache hit/miss reports: not implemented",
    "imports and startup: included in wall time only, not attributed to declarations",
    "tactic timing: aggregated per file from profiler lines, not per declaration",
];

/// Profile one Lean file by running `lean` on an instrumented copy of it.
///
/// `project_root` is the working directory for the run. `timeout_secs` bounds the whole `lean`
/// process. A non-zero Lean exit status is not an error: the report records the error count, and
/// its timings are then incomplete.
pub fn profile_file(project_root: &Path, file: &Path, timeout_secs: u64) -> Result<ProfileReport> {
    if !project_root.is_dir() {
        return Err(
            Error::new(ErrorCode::ProjectNotFound, "project root is not a directory").with_path(project_root),
        );
    }
    let source = fs::read_to_string(file)
        .map_err(|e| Error::new(ErrorCode::Io, format!("reading file: {e}")).with_path(file))?;

    let work = WorkDir::create()?;
    let copy = work.path().join(copy_name(file));
    fs::write(&copy, instrument(&source))
        .map_err(|e| Error::new(ErrorCode::Io, format!("writing instrumented copy: {e}")))?;

    let policy = ProcessPolicy {
        timeout: Duration::from_secs(timeout_secs),
        output_limit_bytes: OUTPUT_LIMIT_BYTES,
        ..ProcessPolicy::default()
    };
    let toolchain = discover_toolchain();
    let result = execute(
        &toolchain,
        &Command::LeanCheck { file: copy },
        Some(project_root),
        &policy,
        &CancelToken::new(),
    )?;
    let out = &result.output;
    if out.timed_out {
        return Err(Error::new(
            ErrorCode::Timeout,
            format!("lean did not finish within {timeout_secs}s"),
        ));
    }
    if out.output_truncated {
        return Err(Error::new(
            ErrorCode::OutputLimitExceeded,
            "lean output exceeded the profiling limit, so the timings would be incomplete",
        ));
    }

    let combined = format!("{}\n{}", out.stdout, out.stderr);
    let analysis = analyze(&combined, &source);
    let lean_errors = result
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    let declared_ms = analysis.declarations.iter().fold(0.0, |acc, d| acc + d.total_ms);
    let mut report = ProfileReport {
        format_version: FORMAT_VERSION,
        file: file.display().to_string(),
        wall_ms: out.duration.as_secs_f64() * 1000.0,
        lean_exit_code: out.exit_code,
        lean_errors,
        declared_ms,
        declarations: analysis.declarations,
        categories: analysis
            .categories
            .into_iter()
            .map(|(k, v)| (k, round_ms(v)))
            .collect(),
        tactics: analysis.tactics,
        suggestions: Vec::new(),
        notes: NOT_MEASURED.iter().map(|s| (*s).to_string()).collect(),
    };
    report.suggestions = suggest(&report);
    if combined.contains("unknown module prefix") || combined.contains("unknown package") {
        report.suggestions.insert(
            0,
            "some imports could not be resolved; set LEAN_PATH to the built project libraries, \
             since this profiler does not run through `lake env`"
                .to_string(),
        );
    }
    Ok(report)
}

/// Insert the profiler options as one line after the header (`prelude`, `module`, `import`
/// lines), so the rest of the file keeps its content. Trace and profiler options apply to the
/// rest of the file.
pub(crate) fn instrument(source: &str) -> String {
    let header = format!(
        "set_option profiler true set_option profiler.threshold {PROFILER_THRESHOLD_MS} \
         set_option trace.profiler true set_option trace.profiler.threshold {TRACE_THRESHOLD_MS}"
    );
    let pieces: Vec<&str> = source.split_inclusive('\n').collect();
    let insert_at = pieces
        .iter()
        .rposition(|line| {
            let t = line.trim();
            t == "prelude" || t == "module" || t == "import" || t.starts_with("import ")
        })
        .map_or(0, |i| i + 1);

    let mut out = String::with_capacity(source.len() + header.len() + 2);
    for piece in &pieces[..insert_at] {
        out.push_str(piece);
    }
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&header);
    out.push('\n');
    for piece in &pieces[insert_at..] {
        out.push_str(piece);
    }
    out
}

fn copy_name(file: &Path) -> String {
    match file.file_name().map(|n| n.to_string_lossy().into_owned()) {
        Some(name) if !name.is_empty() => name,
        _ => "Main.lean".to_string(),
    }
}

/// A uniquely named temporary directory, removed when dropped.
struct WorkDir(PathBuf);

static NEXT_WORK_DIR: AtomicUsize = AtomicUsize::new(0);

impl WorkDir {
    fn create() -> Result<Self> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let seq = NEXT_WORK_DIR.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("tpt-lean-profile-{}-{nanos}-{seq}", std::process::id()));
        fs::create_dir(&path)
            .map_err(|e| Error::new(ErrorCode::Io, format!("creating temp directory: {e}")))?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_goes_after_imports() {
        let src = "import Lean\nimport Foo.Bar\n\ntheorem t : True := trivial\n";
        let out = instrument(src);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "import Lean");
        assert_eq!(lines[1], "import Foo.Bar");
        assert!(lines[2].starts_with("set_option profiler true"));
        assert!(lines[2].contains("trace.profiler.threshold 2"));
        assert_eq!(lines[3], "");
        assert_eq!(lines[4], "theorem t : True := trivial");
        // Exactly one inserted line: later content keeps its original line numbers after an offset of one.
        assert_eq!(out.lines().count(), src.lines().count() + 1);
    }

    #[test]
    fn header_without_imports_goes_first() {
        let out = instrument("def x := 1");
        assert!(out.starts_with("set_option profiler true"));
        assert!(out.ends_with("def x := 1"));
    }

    #[test]
    fn copy_name_falls_back() {
        assert_eq!(copy_name(Path::new("/a/b/Main.lean")), "Main.lean");
        assert_eq!(copy_name(Path::new("/")), "Main.lean");
    }

    #[test]
    fn work_dir_is_unique_and_removed() {
        let a = WorkDir::create().unwrap();
        let b = WorkDir::create().unwrap();
        assert_ne!(a.path(), b.path());
        let kept = a.path().to_path_buf();
        assert!(kept.is_dir());
        drop(a);
        assert!(!kept.exists());
        drop(b);
    }
}
