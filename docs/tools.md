# tpt-lean-tools

`tpt-lean-tools` holds the utilities behind `tpt-lean imports`, `bisect`, `ci` and the doctor
extensions. It is a library. The CLI crate wires the entry points below to commands and output
formats. The crate never writes project files.

## Entry points

| Command | Function | Output |
| --- | --- | --- |
| `imports --unused <file>` | `imports::unused_imports(project_root, file)` | `Vec<Finding>` |
| `imports --suggest "<query>"` | `imports::suggest_imports(project_root, query, limit)` | `Vec<ImportSuggestion>` |
| `imports --minimize <file>` | `imports::minimize_imports(project_root, file)` | `MinimizePlan` (diff only) |
| import cost (heavy imports) | `imports::import_costs(project_root)` | `Vec<ImportCost>` |
| `bisect --good <rev> --bad <rev>` | `bisect::bisect_commits(repo, good, bad, check)` | `BisectReport` |
| bisect check | `bisect::lake_build_check(toolchain, subdir, timeout)` | closure for `bisect_commits` |
| `ci --json --sarif` | `ci::run_ci(project_root, toolchain, &CiOptions)` then `ci::to_sarif(&report)` | `CiReport`, `serde_json::Value` |
| doctor | `doctor::doctor_extensions(project_root)` | `Vec<Check>` |

`Finding` has a stable `code`, a `severity` (`warning`, `info`, `error`), a project-relative
`file`, a 1-based `line`, and a `message`.

## Imports

**Unused imports.** For each `import M` in the file, the module's source is located under the
project root, then under `.lake/packages/<pkg>/`. Declared names of `M` (full names, prefixes and
suffixes of dotted names) are compared with identifier tokens of the file. Import lines, comments
and strings are ignored.

Limits:

- Matching is by identifier text, not elaboration. A name that is resolved through `open`,
  dot-notation, or auto-bound implicits may be missed. Shared prefixes or suffixes count as use,
  so the check errs toward "used".
- Modules that declare `instance`, `notation`, `syntax`, `macro`, `elab`, `attribute`, tactics, or
  any `@[...]` attribute are never reported as unused (`import_implicit`, info). Those are used
  without being named.
- Modules whose source cannot be found (Lean core, Mathlib without `.lake/packages`) produce an
  `import_unresolved` info finding and are not analysed.

**Suggest.** Requires the project index at `.tpt-lean/index.sqlite`. If it is missing, the
function returns `index_error`; run `tpt-lean index refresh` first. Opening the index can migrate
its schema. Matches are exact, prefix, or FTS over declaration names and docstrings.

**Minimize.** Proposes a unified diff that removes every `import_unused` line, with three lines of
context. Nothing is written: `applied` is always `false`. A removed import can break a file that
relied on it for instances or transitive names, so build the project after applying the diff.

**Import cost.** Structural and project-internal only. For each project file it reports direct
imports, external imports (not expanded), the number of project modules reachable through imports
(transitive), and fan-in (direct importers within the project). `heavy` is set when transitive
count is at least 50 or fan-in is at least 20. These are estimates, not compile time: elaboration
cost, proof size, and external packages are not measured. The `.lake` directory is skipped.

## Bisect

Scope: project git commits only.

- `commits_between` runs `git rev-list --reverse good..bad` through the adapter runner with a typed
  argument vector. Revisions are validated (no leading `-`, no `..`, a restricted character set).
- `search` is a binary search over that list with a caller predicate. It assumes failure is
  monotone along the list, and it does not probe the endpoints. The list follows `rev-list`
  order, so merge commits are included and the search is approximate across merges.
- `bisect_commits` creates a detached `git worktree` under the system temp directory for each
  probe, runs the check in it, and removes the worktree on drop (also on error). The user's
  working tree is not touched.
- `lake_build_check` is a typed check: `lake build` in a subdirectory of the checkout, passing on
  exit 0 with no error diagnostics. It needs `lake` and a relative subdirectory without `..`.

Not done: dependency versions, Mathlib updates, compiler flags and import changes. They are listed
in `bisect::NOT_IMPLEMENTED` and in every report's `not_done` field.

## CI

`run_ci` runs `lake build` in the project root through `tpt_lean_adapter::execute`, with the
configured timeout and a sanitised environment. It returns:

- `ok`, `build.status` (`passed`, `failed`, `timeout`, `not_run`), exit code, duration;
- `diagnostics` (parsed Lean output, file paths made relative to the project when possible);
- `failures`: one per error diagnostic, or one `build_failed` / `environment` entry;
- `cache`: placeholder. `available` is `false` and all counters are zero, because no build cache
  is consulted yet.

`to_sarif` builds a SARIF 2.1.0 log: one result per diagnostic with `ruleId` (`lean/error`,
`lean/warning`, `lean/info`), `level`, message, and a physical location. Failures without a file
become `toolExecutionNotifications`. `validate_sarif_shape` checks the keys consumers need
(`version`, `$schema`, `runs[].tool.driver.name`, `results[].ruleId`, `locations`). This is
structural validation only. The official JSON schema is not vendored (licence and size), so the
output is not validated against it.

## Doctor extensions

- **server socket**: reads `.tpt-lean/server.json` (`port`, optional `host`). Only loopback hosts
  are probed, with a 500 ms connect timeout. Missing config is `skipped`.
- **language server**: starts `lake serve` (or `lean --server`) with the adapter LSP client,
  initialises, and shuts down. Skipped when neither `lean` nor `lake` is found. It writes no project
  files, but the server may write its own caches.
- **cache age**: warns when entries in the global cache directory are older than 30 days. Only the
  top level is inspected, and symbolic links are not followed.

Each check has `name`, `status` (`ok`, `warn`, `fail`, `skipped`), `detail` and an optional
`suggestion`.

## Security tests

`tests/security.rs` covers:

- deterministic fuzzing of `WorkspaceRoot::resolve` (about 3000 hostile strings): every accepted
  path must lie under the root, and every rejection must be `path_outside_workspace` or
  `invalid_argument`;
- symlink escape: a link inside the workspace to an outside directory is rejected by `resolve`,
  and the Lean file scanner does not follow it. On Windows, when directory symlinks need
  privileges, the test falls back to a junction (`mklink /J`); if neither can be created, the test
  prints a skip message and returns;
- Windows device names (`NUL`, `CON`, `AUX`, `COM1`, `LPT1`, ...) in any directory;
- process timeout: a sleeping child is killed by `tpt_lean_adapter::run`. The test runs `ping`
  directly. Through a wrapper (`cmd /C ping ...`) the runner kills only `cmd`, the grandchild keeps
  the output pipe open, and the call blocks until the grandchild exits (observed about 30 s). This
  is a runner gap: the adapter should kill the process tree (for example with a Windows job object)
  or stop waiting for pipe EOF after a kill.

## Not done

- `init --mathlib` toolchain note: not implemented. The Mathlib `lean-toolchain` pin changes over
  time and was not verifiable offline, so no pin is hard-coded.
- Bisect over dependency versions, Mathlib updates, compiler flags and import changes.
- Build cache statistics in `ci` (placeholder zeros).
- SARIF JSON-schema validation (structural checks only).
- Memory limits for child processes (the adapter does not enforce them yet).
