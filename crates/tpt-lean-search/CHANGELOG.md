# Changelog

All notable changes to `tpt-lean-search` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

Initial development version `0.1.0`. Nothing has been published yet.

### Added
- Name search: exact full name, exact short name, full-name prefix, short-name prefix.
- Docstring search through the FTS5 index.
- Namespace filter.
- `--near` co-location filter.
- Imported-module preference in ranking (library API).
- Deterministic scores normalized to `0.0..=1.0`.
- Table, JSON, and NDJSON output through the CLI.

### Known limitations
- No recency or dependency-distance ranking factors.
- The CLI does not pass the current file's imports into ranking.
- Type-shaped and goal-based search are not implemented.
