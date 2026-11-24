# Changelog

All notable changes to `tpt-lean-adapter` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

Initial development version `0.1.0`. Nothing has been published yet.

### Added
- Toolchain discovery on `PATH` and in `ELAN_HOME/bin`.
- Typed `lean` and `lake` invocations with no shell.
- Build target validation.
- Process runner with timeout, output limit, cancellation, and environment sanitization.
- Diagnostic parsing into severity, file, line, column, and message.
- Persisted last-run diagnostics.
- LSP client over stdio for `lake serve` and `lean --server`, with document sync, diagnostics,
  plain goals, restart, and cancellation.
- Lemma suggestions from the index.
- Version parsing for `lean` and `lake`.

### Known limitations
- Memory limits are enforced by a sampling watchdog (not OS job objects or rlimits); overshoot is possible between samples.
- Lean version compatibility is not checked.
- The Lean sidecar covers one module's declarations at a time and does not emit goals. Modules
  must be built first; the index stores the results (`index sidecar`).
