# Changelog

All notable changes to `tpt-lean-build` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

Initial development version `0.1.0`. Nothing has been published yet.

### Added
- `lake build` orchestration through the adapter, with typed arguments and no shell.
- Structured JSON event stream (`build_started`, cache events, file events, diagnostics,
  `build_finished`).
- Cache keys from a project fingerprint.
- Disk cache and in-memory cache of build records, with status and pruning.
- Project scanning and Lake manifest reading.
- Watch mode that skips unchanged inputs.
- Tool probing and Lake output parsing.
- Slowest-file explanation.

### Known limitations
- No live streaming of Lake output.
- No shared team or CI caches.
- No parallel orchestration beyond Lake's own job pool.
