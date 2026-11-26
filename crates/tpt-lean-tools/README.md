# tpt-lean-tools

Library utilities built on the [tpt-lean](https://github.com/tpt-solutions/tpt-lean) components:
import analysis, sorry reporting, the toolchain upgrade plan, git commit bisect, a CI runner with
SARIF output, and the extra checks and fixes behind `tpt-lean doctor`.

Most functions here are read-only. The exceptions are the explicit write paths: `upgrade` changes
`lean-toolchain` only when applied, and `fix` rebuilds the symbol index.

## Status

Early development (`0.1.0`, not yet published). Import analysis, sorry reporting, the upgrade
plan, bisect, the CI runner, the doctor checks and fixes are implemented and tested. Each part has
documented limits.

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

### Sorry report (`sorry`)

- Finds every `sorry` and `admit` per declaration. It works on lexer tokens, so comments and
  strings are ignored. It is textual: it does not show goals.
- Backs `tpt-lean sorry-report [paths] [--deny-sorry]`.

### Upgrade (`upgrade`)

- Plans a change of the `lean-toolchain` file to a new version. It reports the Mathlib pin and the
  manifest that must be updated by hand.
- With `apply`, it runs `lake build` and restores the original file if the build fails.

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
- `validate_sarif_shape` checks structure only. The check against the official OASIS schema is
  `scripts/validate-sarif.py`, which runs outside the crate.

### Doctor extensions (`doctor`) and fixes (`fix`)

- `doctor_extensions` runs the extra checks: server socket health, language server health,
  build status, and global cache age (warning after `CACHE_WARN_DAYS`).
- The language server check starts `lake serve` or `lean --server` and shuts it down again.
- The checks are read-only with respect to project files.
- `fix` (`doctor --fix`) prints a plan, then rebuilds the symbol index when it is missing or
  damaged. A damaged `index.sqlite` is deleted first, because the index is derived data. Other
  problems are reported only.
- Symbolic links are not followed while scanning.

## Usage

These are exposed through the CLI:

```
tpt-lean imports --unused <file>
tpt-lean imports --suggest "<query>"
tpt-lean imports --minimize <file>
tpt-lean sorry-report [paths] [--deny-sorry]
tpt-lean upgrade --to VERSION [--apply]
tpt-lean bisect --good REV --bad REV [--timeout S]
tpt-lean ci [--json] [--sarif] [--timeout S]
tpt-lean doctor [--fix]
```

```toml
[dependencies]
tpt-lean-tools = "0.1"
```

## Limits

- Import analysis is textual, not semantic.
- Bisect is approximate across merges and assumes monotone failure.
- SARIF output is checked structurally in the crate. The official schema check is in
  `scripts/validate-sarif.py`.
- CI cache statistics are placeholders.
- Memory limits for child processes are enforced by the sampling watchdog in `tpt-lean-adapter`
  (see that crate's limits).

## Dependencies

| Crate | Purpose |
| --- | --- |
| `tpt-lean-core` | configuration, errors, paths |
| `tpt-lean-syntax` | parsing for import analysis and sorry reporting |
| `tpt-lean-index` | module lookup for suggestions and the index fix |
| `tpt-lean-adapter` | running `git` and `lake` through typed invocations |
| `tpt-lean-build` | build status for the CI runner and doctor |
| `tpt-lean-fmt` | formatting support |
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
