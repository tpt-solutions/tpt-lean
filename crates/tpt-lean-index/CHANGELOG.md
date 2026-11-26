# Changelog

All notable changes to `tpt-lean-index` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

Initial development version `0.1.0`. Not yet published; see [PUBLISHING.md](../../PUBLISHING.md).

### Added
- SQLite index in WAL mode with bundled SQLite (`rusqlite`).
- Tables: `packages`, `files`, `symbols`, `imports`, `symbol_text` (FTS5), `diagnostics`,
  `build_runs`, `profile_runs`, `cache_entries`.
- Schema migrations, one version at a time, each in its own transaction. A database written by a
  newer tpt-lean is refused.
- File scanning of the project and local `.lake/packages`, skipping `.lake`, `build`, and
  `target`.
- `.gitignore` files in the project and its subdirectories are honoured (`ignore` module).
- Incremental refresh by content hash. Measured: one edit in a 500-file project refreshes in p50
  27 ms (a full refresh takes 372 ms).
- Import graph edges. Invalidation follows the import closure.
- Lean sidecar declarations per module (`store_declarations`, `modules_needing_sidecar`), keyed
  by source hash, import-closure hash, toolchain and sidecar schema. Stale rows are dropped on
  every refresh.
- FTS5 full-text search over names and docstrings, with query sanitization.
- Status reporting and the index health check used by `tpt-lean doctor`.
- Scan, refresh, status, and clear operations.

### Known limitations
- Imports from outside the project (Lean core, Mathlib) are not hashed. A change there is caught
  only by the toolchain key, so run `index clear` after changing dependency versions.
- Docstrings and dependency edges are stored in the main tables, not in separate tables.
- Goals and per-project symbol tables from Lean are not stored.
