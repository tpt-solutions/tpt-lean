# Changelog

All notable changes to `tpt-lean-tools` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

Initial development version `0.1.0`. Nothing has been published yet.

### Added
- Import analysis: unused imports, import suggestions, minimization plans, and import cost
  estimates.
- Git commit bisect with temporary worktrees and a `lake build` check.
- CI runner with structured findings and SARIF 2.1.0 output.
- Structural SARIF validation.
- Doctor extensions: server socket, language server, build status, and cache age checks.

### Known limitations
- Import analysis matches identifier text, not elaborated names.
- Bisect does not cover dependency versions, Mathlib updates, compiler flags, or import changes.
- SARIF output is not validated against the official schema.
- CI cache statistics are placeholders.
