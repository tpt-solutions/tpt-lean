# Changelog

All notable changes to `tpt-lean-build` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

Initial development version `0.1.0`. Not yet published; see [PUBLISHING.md](../../PUBLISHING.md).

### Added
- `lake build` orchestration through the adapter, with typed arguments and no shell.
- Structured JSON event stream (`build_started`, cache events, file events, diagnostics,
  `build_finished`), emitted live as Lake runs.
- Cache keys from a project fingerprint; `key_ms` in the JSON output.
- Disk cache and in-memory cache of build records, with status and pruning.
- Shared cache layer: `TPT_LEAN_SHARED_CACHE` names directories of build records. Lookups go
  memory, local disk, then shared; shared hits are copied locally.
- Build plan (`build --plan [--json]`): module import waves, critical path, and widest wave.
- Build report (`build --report`): the cache layer that served the build, cache-key time, Lake's
  share of the total, and the ten slowest files.
- Project scanning and Lake manifest reading.
- Watch mode that skips unchanged inputs.
- Tool probing and Lake output parsing.
- Slowest-file explanation.

### Known limitations
- Parallel orchestration beyond Lake's own job pool is not implemented.
- Measured, not explained: a full build of a 41-module project takes 17 to 20 s, about 2.5 times
  `lake build` from the same shell (about 7 s). The slowdown follows the parent process of Lake.
- Measured, not explained: a cache-hit build takes about 2.6 s on a small project, against about
  1.1 s for a direct `lake build` that does nothing.
