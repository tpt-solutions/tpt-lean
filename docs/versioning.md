# Versioning policy

tpt-lean has four versions that move independently. Each is stated below with the rule for when it
changes.

## 1. Crate versions (semver)

All workspace crates share one version, set in `[workspace.package]` in `Cargo.toml`. The project
is pre-release (`0.1.x`). Until `1.0.0`:

- A minor bump (`0.1` to `0.2`) may break Rust APIs, CLI flags, or config keys. Each break is
  listed in `CHANGELOG.md` under the release.
- A patch bump (`0.1.0` to `0.1.1`) fixes bugs and adds nothing that changes existing behaviour.

From `1.0.0`, standard semver applies to the Rust crates' public APIs.

No crate is published yet. Whether to publish to crates.io is an open decision (see
[todo.md](../todo.md), Phase 9).

## 2. C ABI version

`tptl_abi_version()` returns the ABI version, which is separate from the crate version. It is `1`
today, and [ffi.md](ffi.md) documents the export list.

- A breaking ABI change bumps the ABI version: removing or renaming an export, changing a
  signature or struct layout, changing a status code's value, or changing ownership rules.
- Adding a new export does not bump the ABI version. Callers should check that the symbol exists
  before using it.
- The header `crates/tpt-lean-ffi/include/tpt_lean.h` and the exported symbols must match; a test
  (`header_and_exports_match`) enforces this.

## 3. JSON output schema

Every machine-readable response carries `schema_version` (currently `1`, in
`tpt-lean-core::schema`).

- Field names are stable. New fields may be added; existing fields are never renamed or removed
  within a schema version.
- A change that renames, removes, or changes the meaning of a field bumps `schema_version`.
- Clients should ignore unknown fields.

The same rule applies to the API server and MCP tool output schemas.

## 4. Lean version support

tpt-lean does not pin a Lean version for users. It records the Lean version reported by the
language server and reports whether that version is in the tested set. Only features that depend on
a specific Lean behaviour are gated, and none are gated today; `explain` warns when the version is
untested. `tpt-lean doctor` lists the tested versions and reports pin mismatches.

Current tested set: Lean `4.36.0`. Testing against the previous stable Lean release is not done yet.

## Deprecation (proposed)

Proposed, not yet adopted: a deprecated CLI flag, config key, or JSON field keeps working for at
least one minor release and prints a warning on stderr, and is removed only in a release whose
changelog says so.
