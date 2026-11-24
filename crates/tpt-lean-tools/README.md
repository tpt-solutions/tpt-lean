# tpt-lean-tools

Library utilities built on the [tpt-lean](https://github.com/tpt-solutions/tpt-lean) components:
import analysis, git commit bisect, a CI runner with SARIF output, and extra checks for
`tpt-lean doctor`.

Everything here is a library entry point. The CLI crate decides how to wire these up. Nothing in
this crate writes project files.

## Status

Early development (`0.1.0`, not yet published). Import analysis, bisect, the CI runner, and the
doctor checks are implemented and tested. Each part has documented limits.

## Modules

### Imports (`imports`)

- `unused_imports` reports imports whose names are not used in the file. Matching is by
  identifier text, not elaboration, so a name that comes in through `open` may be reported
  wrongly.
- `suggest_imports` ranks modules that declare a name, using the index.
- `minimize_imports` returns a `MinimizePlan` describing a smaller import list. It is a diff
  only; nothing is written.
- `import_costs` estimates the cost of each import from the import graph: external imports
  (not expanded), the number of project modules reachable through imports, and fan-in. Results
  are flagged as heavy above fixed thresholds (`HEAVY_TRANSITIVE`, `HEAVY_FAN_IN`). These are
  estimates, not compile times. Proof size, elaboration cost, and external packages are not
  measured.
- Modules whose source cannot be found (Lean core, or Mathlib without `.lake/packages`) produce
  an `import_unresolved` info finding and are not analysed. The `.lake` directory is skipped.

### Bisect (`bisect`)

- `commits_between` lists the commits between a good and a bad revision with `git rev-list`,
  through the adapter's typed runner. Revisions are validated.
- `search` runs a binary search over that list with a caller-supplied check. It assumes the
  failure is monotone along the list. Merge commits are included, so the result is approximate
  across merges.
- `bisect_commits` checks out each probe in a temporary detached worktree, runs the check, and
  removes the worktree afterwards, including on error. Your working tree is not touched.
- `lake_build_check` is a ready-made check: `lake build` in a subdirectory of the checkout,
  passing on exit code 0 with no error diagnostics.

Not bisected: dependency versions, Mathlib updates, compiler flags, and import changes. They are
listed in `bisect::NOT_IMPLEMENTED` and in every report's `not_done` field.

### CI (`ci`)

- `run_ci` builds the project, collects diagnostics and failure summaries, and returns a
  `CiReport`. Cache statistics are zeros for now.
- `to_sarif` converts findings to SARIF 2.1.0. Each `Finding` has a stable machine-readable code,
  a severity (`warning`, `info`, or `error`), a project-relative path with forward slashes, and a
  1-based line (0 when not tied to a line).
- `validate_sarif_shape` checks structure only. The official SARIF JSON schema is not vendored
  (licence and size), so output is not validated against it.

### Doctor extensions (`doctor`)

- `doctor_extensions` runs the extra checks: server socket health, language server health,
  build status, and global cache age (warning after `CACHE_WARN_DAYS`).
- The language server check starts `lake serve` or `lean --server` and shuts it down again.
- Checks are read-only with respect to project files.
- Symbolic links are not followed while scanning.

## Usage

These are exposed through the CLI:

```
tpt-lean imports --unused <file>
tpt-lean imports --suggest "<query>"
tpt-lean imports --minimize <file>
tpt-lean bisect --good REV --bad REV [--timeout S]
tpt-lean ci [--json] [--sarif] [--timeout S]
tpt-lean doctor
```

```toml
[dependencies]
tpt-lean-tools = "0.1"
```

## Limits

- Import analysis is textual, not semantic.
- Bisect is approximate across merges and assumes monotone failure.
- SARIF is checked structurally only.
- Memory limits for child processes are not enforced (see `tpt-lean-adapter`).

## Dependencies

| Crate | Purpose |
| --- | --- |
| `tpt-lean-core` | configuration, errors, paths |
| `tpt-lean-syntax` | parsing for import analysis |
| `tpt-lean-index` | module lookup for suggestions |
| `tpt-lean-adapter` | running `git` and `lake` through typed invocations |
| `serde`, `serde_json` | reports and SARIF output |

## Testing

```
cargo test -p tpt-lean-tools
```

The crate has unit tests. `tests/security.rs` covers the path and revision checks, including
symbolic link handling.

## Related documentation

- [Tools guide](https://github.com/tpt-solutions/tpt-lean/blob/master/docs/tools.md)

## License

Licensed under either of [MIT](https://opensource.org/licenses/MIT) or
[Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0), at your option.
