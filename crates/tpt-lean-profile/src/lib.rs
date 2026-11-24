// SPDX-License-Identifier: MIT OR Apache-2.0
//! Profiling of Lean files: per-file wall time, per-declaration timing from Lean's own
//! `trace.profiler` output, tactic totals, slowest-declaration ranking, JSON reports, and
//! comparison of two reports.
//!
//! Not implemented: memory tracking, flamegraphs, slow-import detection, dependency graphs,
//! build-step and cache reports. Each is listed in [`ProfileReport::notes`].

mod analysis;
mod report;
mod run;
mod trace;

pub use report::ImportTiming;
pub use report::{
    compare, format_ms, from_json, read_report, render_report, slowest, suggest, to_json, DeclTiming,
    ProfileReport, TacticTiming, FORMAT_VERSION,
};
pub use run::{import_timings, profile_file, profile_file_traced};

/// Folded stacks for the trace tree in Lean's combined output, ready for flamegraph.pl or inferno.
/// Empty when the output has no trace nodes.
pub fn flamegraph_folded(output: &str) -> String {
    trace::folded_stacks(&trace::parse_trace(output))
}
