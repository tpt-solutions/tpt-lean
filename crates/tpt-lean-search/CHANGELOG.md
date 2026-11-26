# Changelog

All notable changes to `tpt-lean-search` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

Initial development version `0.1.0`. Not yet published; see [PUBLISHING.md](../../PUBLISHING.md).

### Added
- Name search: exact full name, exact short name, full-name prefix, short-name prefix.
- Docstring search through the FTS5 index.
- Namespace filter and `--near` co-location filter.
- Imported-module preference (`--from FILE` in the CLI).
- Type-shaped search (`--type PATTERN`) over sidecar declaration types.
- Goal-based search (`--goal-at FILE:LINE:COL`), using the Lean adapter.
- Deterministic scores normalized to `0.0..=1.0`.
- Table, JSON, and NDJSON output through the CLI.
- Benchmark (`tests/bench.rs`, run with `--release -- --ignored`): on 5,000 symbols, search p50
  0.99 ms and p95 4.0 ms.

### Known limitations
- No recency or dependency-distance ranking factors.
