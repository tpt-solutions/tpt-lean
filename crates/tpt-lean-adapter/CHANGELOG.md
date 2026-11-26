# Changelog

All notable changes to `tpt-lean-adapter` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

Initial development version `0.1.0`. Not yet published; see [PUBLISHING.md](../../PUBLISHING.md).

### Added
- Toolchain discovery on `PATH` and in `ELAN_HOME/bin`.
- Typed `lean` and `lake` invocations with no shell. Build target validation.
- Process runner with timeout, output limit, cancellation, environment sanitization, and a
  sampling memory watchdog.
- `run_streaming`: passes output lines to the caller as they are read. `tpt-lean build` uses it
  to report Lake's progress while the build runs.
- Diagnostic parsing into severity, file, line, column, and message.
- Persisted last-run diagnostics.
- LSP client over stdio for `lake serve` and `lean --server`, with document sync, diagnostics,
  plain goals, restart, cancellation, and `$/lean/fileProgress` counts.
- Lean sidecar (`lean/tpt-lean-sidecar`): declarations (name, kind, type, docstring) as versioned
  JSON, and a `goals FILE LINE COL` mode that prints the goals before the innermost tactic at a
  position, read from Lean's info trees. `tpt-lean goals --sidecar` uses it without a language
  server.
- Lemma suggestions from the index.
- Version parsing for `lean` and `lake`.
- Tested against Lean 4.35.0 and 4.36.0; the full suite passes on both locally. The sidecar builds
  per version, and docstrings are trimmed because 4.35 keeps a trailing space that 4.36 drops.

### Fixed
- Lake 5 prints Lean's errors as `error: FILE:LINE:COL: message`. The diagnostic parser did not
  read that form, so `build` and `ci` reported a failed build with no diagnostics. Both orders
  parse now.

### Known limitations
- Memory limits are enforced by a sampling watchdog (not OS job objects or rlimits). It checks
  every 100 ms, so overshoot is possible between samples.
- Only Lean 4.35.0 and 4.36.0 are tested.
- The sidecar covers one module at a time and needs its modules built first; the index stores the
  results (`index sidecar`).
