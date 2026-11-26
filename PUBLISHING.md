# Publishing

This file tracks which tpt-lean crates are on crates.io, the order they must be published in,
and what blocks each one. Update it in the same commit as every release.

Nothing has been published yet. Every crate is at version `0.1.0` (set once, in
`[workspace.package]` and `[workspace.dependencies]` in the root `Cargo.toml`).

## Status

"Tier" is the publish order: a crate can be published only after every crate in a lower tier
is on crates.io. Crates in the same tier can go in any order.

| Tier | Crate | Publishable | Internal dependencies | Status |
| --- | --- | --- | --- | --- |
| 1 | `tpt-lean-core` | yes | none | not published |
| 1 | `tpt-lean-syntax` | yes | none | not published |
| 1 | `tpt-lean-pipe` | **no** (`publish = false`) | none | not published; blocks `tpt-lean-server` (see Blockers) |
| 2 | `tpt-lean-fmt` | yes | `syntax` | not published |
| 2 | `tpt-lean-index` | yes | `core`, `syntax` | not published |
| 3 | `tpt-lean-search` | yes | `core`, `index` | not published |
| 4 | `tpt-lean-adapter` | yes | `core`, `index`, `search`, `syntax` | not published |
| 5 | `tpt-lean-profile` | yes | `adapter`, `core`, `syntax` | not published |
| 5 | `tpt-lean-build` | yes | `adapter`, `core`, `syntax` | not published |
| 6 | `tpt-lean-docgen` | yes | `adapter`, `core`, `index`, `syntax` | not published |
| 6 | `tpt-lean-lsp` | yes | `adapter`, `core`, `fmt`, `index`, `syntax` | not published |
| 6 | `tpt-lean-tools` | yes | `adapter`, `build`, `core`, `fmt`, `index`, `syntax` | not published |
| 6 | `tpt-lean-ffi` | yes | `adapter`, `core`, `fmt`, `index`, `search`, `syntax` | not published |
| 7 | `tpt-lean-mcp` | yes | `adapter`, `core`, `fmt`, `index`, `profile`, `search`, `syntax` | not published |
| 8 | `tpt-lean-server` | yes | `adapter`, `build`, `core`, `fmt`, `index`, `profile`, `search`, `syntax`, `tools`; `pipe` on Windows | not published; blocked by `tpt-lean-pipe` |
| 9 | `tpt-lean-cli` | yes | `core`, `syntax`, `index`, `search`, `fmt`, `adapter`, `mcp`, `server`, `docgen`, `profile`, `build`, `lsp`, `tools` | not published; ships the `tpt-lean` and `tlean` binaries |
| - | `tpt-lean-wasm` | **no** (`publish = false`) | `fmt`, `syntax` | distributed as an npm package only; never publish to crates.io |

Dependencies were read from each crate's `Cargo.toml`. Re-check them when a crate's dependencies
change, because the tier of every crate above it may move.

## Blockers

1. **`tpt-lean-pipe` is not publishable, but `tpt-lean-server` needs it.** The server depends on
   `tpt-lean-pipe` under `[target.'cfg(windows)'.dependencies]`. Cargo resolves a published
   crate's dependencies from crates.io, so `tpt-lean-server` cannot be published while
   `tpt-lean-pipe` is `publish = false`.
   *Decision needed.* Either (a) set `publish = true` on `tpt-lean-pipe` and publish it in tier 1
   (it now has its own README and CHANGELOG), or (b) move the Windows named-pipe code into
   `tpt-lean-server`. Recommendation: (a).

2. **License texts are not inside the crates.** `LICENSE-MIT` and `LICENSE-APACHE` are at the
   repository root only. `cargo package` includes only files under each crate's directory, so
   the published crates would name the license in `Cargo.toml` but not ship its text.
   *Decision needed.* Copy both files into each publishable crate, or add them with `include`.
   Not done yet.

3. **Crate names are not checked against crates.io.** This file was written without network
   access. Check each name is free before its first publish.

## Release checklist

1. Start from a clean tree on `master`. Run `cargo test --workspace`, `cargo clippy --workspace`,
   and `cargo deny check`.
2. Choose the version. Set it in `[workspace.package] version` and in every `version = "..."`
   under `[workspace.dependencies]`. Path dependencies need the version to publish.
3. In each crate's CHANGELOG, rename `## [Unreleased]` to `## [X.Y.Z] - YYYY-MM-DD` and add an
   empty `## [Unreleased]` above it. Do the same in the root `CHANGELOG.md`.
4. Check the file list for each crate: `cargo package -p NAME --list`. A full
   `cargo package` or `cargo publish --dry-run` for a crate in tier 2 or higher fails until its
   dependencies are on crates.io. That is expected, so check tiers in order.
5. Publish in tier order with `cargo publish -p NAME`. Wait until crates.io shows a tier before
   publishing the next one, which usually takes a minute or two.
6. Add a row to the Release log below, and update the Status column above.
7. Tag the release `vX.Y.Z` and push the tag. The `v*` tag starts
   `.github/workflows/release.yml`, which builds the binaries and `SHA256SUMS`. That workflow has
   not run on GitHub yet, so check it before tagging.

Publishing is permanent for each version: a version can be yanked (hidden) but not deleted, and
its number cannot be reused. Log in with `cargo login` yourself; the token is not stored in this
repository.

## Release log

| Date | Crate | Version | Notes |
| --- | --- | --- | --- |
| - | none yet | - | - |
