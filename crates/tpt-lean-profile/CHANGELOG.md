# Changelog

All notable changes to `tpt-lean-profile` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

Initial development version `0.1.0`. Nothing has been published yet.

### Added
- Per-file profiling by running Lean with trace output.
- Parsing of Lean's profiler summary and trace output into declaration and tactic timings.
- Slowest-declaration ranking, tactic totals, and call-tree subtrees.
- Suggestions based on timings.
- Versioned JSON report format with `read_report` and `to_json`/`from_json`.
- Human-readable report rendering.
- Comparison of two reports.

### Known limitations
- No memory tracking, flamegraphs, slow-import detection, dependency graphs, or build-step and
  cache reports. Each report's `notes` lists these.
- Memory and flamegraph output are not available in the MCP tool either.
