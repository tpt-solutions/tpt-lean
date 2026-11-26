# Changelog

All notable changes to `tpt-lean-tools` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

Initial development version `0.1.0`. Not yet published; see [PUBLISHING.md](../../PUBLISHING.md).

### Added
- Import analysis: unused imports, import suggestions, minimization plans, and import cost
  estimates.
- Sorry report: every `sorry` and `admit` per declaration, found on lexer tokens.
- Upgrade plan (`upgrade --to VERSION [--apply]`): a `lean-toolchain` change with the Mathlib pin
  and manifest to update. With `--apply` it runs `lake build` and restores the file if the build
  fails. Tested here: dry run, rollback on a real Lean error, and a move to v4.36.0.
- Git commit bisect with temporary worktrees and a `lake build` check.
- CI runner with structured findings and SARIF 2.1.0 output.
- SARIF validation: `scripts/validate-sarif.py` checks `ci --sarif` output against the OASIS
  schema. The crate's own check (`validate_sarif_shape`) is structural.
- Doctor extensions: server socket, language server, build status, and cache age checks.
- Doctor fix (`doctor --fix`): rebuilds the symbol index when it is missing or damaged.

### Known limitations
- Import analysis matches identifier text, not elaborated names.
- Bisect does not cover dependency versions, Mathlib updates, compiler flags, or import changes.
- The crate checks SARIF structure only; the official schema check runs from `scripts/`.
- CI cache statistics are placeholders.
