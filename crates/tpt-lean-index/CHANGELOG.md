# Changelog

All notable changes to `tpt-lean-index` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

Initial development version `0.1.0`. Nothing has been published yet.

### Added
- SQLite index in WAL mode with bundled SQLite (`rusqlite`).
- Schema version 1: `packages`, `files`, `symbols`, `imports`, `symbol_text` (FTS5),
  `diagnostics`, `build_runs`, `profile_runs`, `cache_entries`.
- File scanning of the project and local `.lake/packages`, skipping `.lake`, `build`, and
  `target`.
- Incremental refresh by content hash.
- Import graph edges.
- FTS5 full-text search over names and docstrings, with query sanitization.
- Status reporting and an index health check used by `tpt-lean doctor`.
- Scan, refresh, status, and clear operations.

### Known limitations
- No `.gitignore` parsing.
- Invalidation does not follow the import graph.
- No schema upgrade steps; a version mismatch needs a rebuild of the index.
