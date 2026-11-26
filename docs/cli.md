# CLI reference

The binary is `tpt-lean`. `tlean` is the same program under a shorter name. Every command accepts
the global flags below. Commands that run Lean need `lean` and `lake` on `PATH`, or installed by
elan in `~/.elan/bin`.

For a first run, see [quickstart.md](quickstart.md). For the JSON output, see
[json-schemas.md](json-schemas.md).

## Global flags

| Flag | Meaning |
| --- | --- |
| `--json` | print one JSON envelope (`schema_version`, `tool`, `kind`, `ok`, `data`) |
| `--ndjson` | print one JSON record per line for list results |
| `--project <dir>` | project root. Default: the nearest directory above the working directory with `tpt-lean.toml` or a `lakefile` |
| `--set section.key=value` | configuration override (highest precedence), repeatable |
| `-v`, `--verbose` | debug logging to stderr (or set `TPT_LEAN_LOG`) |
| `-q`, `--quiet` | errors only |

Flags can be given before or after the command name.

## Positions

`FILE:LINE:COL` uses a 1-based line and a 1-based column that counts characters. The file is
relative to the project root.

## Configuration

Settings are read in this order, later ones winning: built-in defaults, `tpt-lean.toml`,
`.tpt-lean/config.toml`, environment variables (`TPT_LEAN_<SECTION>_<KEY>`), then `--set`.
`tpt-lean config show` prints the effective result.

The `[security]` section controls what the MCP server and API server may do:
`allow_write`, `allow_build` (default true), `allow_profile`, and `command_timeout_seconds`
(default 120). See [security.md](security.md).

The `[search]` section sets the optional ranking factors of `search` (see below). All three
weights default to 0, which keeps the default ordering. Set them in `.tpt-lean/config.toml` or
`tpt-lean.toml`, or with `--set search.recency_weight=2.5`; environment variables are not read for
these keys.

```toml
[search]
recency_weight = 5.0          # bonus for a recently changed file, scaled by 0.5^(age / half life)
recency_half_life_days = 30.0 # age at which the recency bonus halves
distance_weight = 5.0         # bonus of weight / (1 + hops) along import edges from the origin
```

Weights are in the same raw points as the name and kind bonuses (a prefix match adds 40 to 50 points;
the score is capped at 1 after dividing by 200). Negative weights, a non-finite value, and a half life
that is not positive are configuration errors.

The `[lsp]` section sets the language server behind `tpt-lean lsp`:
`idle_timeout_seconds` (default 900; the server is shut down after that many seconds without an
editor request, and 0 disables this) and `health_timeout_seconds` (unset by default, which means
`security.command_timeout_seconds`; a request with no answer in that time restarts the server and
fails with a timeout). Environment variables: `TPT_LEAN_LSP_IDLE_TIMEOUT_SECONDS` and
`TPT_LEAN_LSP_HEALTH_TIMEOUT_SECONDS`. See [lsp.md](lsp.md).

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | success |
| 1 | a check failed: `fmt --check` would change files, `ci` failed, or Lean reported errors |
| 2 | invalid argument, unknown command, or unknown flag |
| 3 | Lean, Lake or the project was not found |
| 4 | configuration is invalid |
| 5 | a path is outside the project |
| 6 | index error |
| 7 | the syntax is invalid or uncertain (the formatter refused) |
| 8 | timeout or output limit exceeded |
| 9 | I/O error |
| 70 | internal error |

With `--json`, a failure prints an error envelope (see [json-schemas.md](json-schemas.md)) and uses
the same exit code.

## Project and environment

- `init <dir> [--mathlib] [--template NAME]`: create a starter project with `lakefile.lean`,
  `lean-toolchain`, `Main.lean`, `tpt-lean.toml`, `.gitignore` and `README.md`. Templates:
  `default`, `theorem`, `teaching`, `library`, and `mathlib` (the same as `--mathlib`). With
  `--mathlib`, the project requires Mathlib at the tag matching the tested Lean release (`v4.36.0`)
  and pins `lean-toolchain` to it. A full Mathlib build is not run by `init`. The directory name
  may use letters, digits, `_` and `-`.
- `doctor [--fix]`: checks Lean and Lake, the toolchain pin, the project configuration, the dependency
  cache, Mathlib, write permissions, the index, the cache, the server socket, and whether the
  language server starts and stops. A `lean-version` check warns when the Lean on `PATH` differs
  from the pin, or is not one of the tested versions (currently 4.35.0 and 4.36.0). Exits 3 when a
  check fails.
  The `config-file` check warns when there is no configuration file. The `sidecar` check reports
  the Lean sidecar: `TPT_LEAN_SIDECAR` when set, or the build in `lean/tpt-lean-sidecar` when this
  is a checkout of the tool itself.
  `--fix` prints a plan first, then applies the fixes it can make safely. The fixes are: build the
  index when it is missing, and when its database is damaged delete the `index.sqlite` file (and its
  `-wal` and `-shm` files) and rebuild it from the sources; write the default configuration to
  `.tpt-lean/config.toml` when there is none (it records today's defaults, so later default changes
  do not reach it); and run `lake build` in `lean/tpt-lean-sidecar` when that source is present but
  not built. Anything else, such as a missing toolchain or a broken configuration, is reported and
  not changed. Each applied fix is listed as a `fix` check.
- `setup [--editor NAME]... [--mcp NAME]... [--dry-run]`: writes the editor and MCP client config
  for the project. Editors: `helix` (merges into `.helix/languages.toml`), `vscode`
  (`.vscode/tpt-lean-lsp.json`), `neovim` and `emacs` (`.tpt-lean/editors/`). MCP clients: `claude`
  (`.mcp.json`) and `cursor` (`.cursor/mcp.json`). Every change is shown as a diff first. Existing
  files are merged: other servers are kept, and an existing `tpt-lean` entry that differs is kept too
  and reported. Files written by setup are created only when absent. Merged JSON is written with its
  keys sorted. With `--dry-run` nothing is written and the exit status is 1 when a change is
  pending, so a CI job can check for drift.
- `completions bash|zsh|fish|powershell`: print a completion script for `tpt-lean` and `tlean`.
  For example `tpt-lean completions bash > ~/.local/share/bash-completion/completions/tpt-lean`.
- `config show`: the effective configuration.
- `clean [--timeout S]`: runs `lake clean`.
- `cache status`: the location and size of the global cache, and the shared layers named by
  `TPT_LEAN_SHARED_CACHE`.
- `cache prune [--days N] [--dry-run]`: deletes old files in the global cache (default 30 days).
- `cache export DIR` / `cache import DIR`: copy build records to or from `DIR`, for CI artifacts.
  An import skips files that are not valid records.

## Source

- `fmt [--check] [--diff] [--verify] [paths...]`: the conservative formatter. With no paths it
  covers the project. `--check` exits 1 when a file would change. `--diff` prints the changes.
  `--verify` also asks Lean to confirm the formatted text has no new errors, and writes only if it
  does. Files with tabs, or with syntax the formatter cannot read, are refused.
- `imports [file]`: the `import` lines of one file, or of the whole project.
- `imports --unused FILE`: a heuristic. An import is flagged when none of the module's declared
  names appear in the file. Modules with instances, notations, tactics or attributes are never
  flagged.
- `imports --suggest QUERY [--limit N]`: modules that declare names matching `QUERY`. Needs an index.
- `imports --minimize FILE`: the proposed import removals, as a diff. The file is not written.
- `imports --cost`: direct and transitive import counts per module. This is a structural estimate,
  not a compile time.
- `imports --cost --timed FILE [--timeout S]`: the compile-time estimate per import of one file. Lean
  loads each direct import on its own and the extra wall time is reported per import (the same
  timings as `profile --imports`; one run per import, so treat small differences as noise). Each
  entry also shows the module's transitive project import count, when the module is a project file.
- `sorry-report [paths...] [--deny-sorry] [--sidecar [--exe PATH] [--timeout S]]`: lists every
  `sorry` and `admit` with its file, line, column and the declaration it is in. Only code is
  searched: words inside comments and strings are ignored. Without paths it covers the project (not
  `lakefile.lean`). `--deny-sorry` exits 1 when there is any. The scan itself does not elaborate the
  file. With `--sidecar` each hit also gets the goal at its position from the Lean sidecar, in one
  elaboration per file (see the `goals` section for the targets); a file the sidecar cannot elaborate
  gets a `goals_error` instead.
- `fix FILE [--apply] [--timeout S]`: plans three mechanical fixes for one file and prints the diff:
  remove imports that nothing in the file uses; add `import M` for an unknown identifier that exactly
  one indexed module declares (Lean names the identifier; the index says where it is declared; an
  identifier with no single module is listed as unresolved); and format the file, when the formatter
  can read it. Nothing is written without `--apply`. With `--apply` the file is written, Lean is run
  on it again, and if Lean reports more errors than before the original text is restored and the exit
  status is 1. Needs Lean; the unknown-identifier step needs an index.
- `search <query> [--from FILE] [--namespace NS] [--near TERM]... [--limit N]`: names and
  docstrings in the index. `--from FILE` ranks symbols from modules that the file imports higher.
  `--near` keeps only files that also declare a name matching each term.
  Two more factors apply when their weights in `[search]` are non-zero. Recency: a file whose
  contents changed more recently in the index (the time the index stored the change, not git time)
  ranks higher. Distance: a project module closer to the origin ranks higher, where the origin is
  the module of the `--from` file, and when there is no `--from` the distance is measured from the
  project's root modules (those that no project module imports). With no origin and no project
  modules in the index, the distance factor adds nothing. The MCP tools, the API server and the FFI
  use zero weights, so their ordering is unchanged.
- `search --type PATTERN [--namespace NS] [--limit N]`: declarations whose type contains every
  identifier of the pattern as a whole token, for example `Nat → Nat` or `List Nat`. Types come from
  `index sidecar`, so this needs Lean. Arrows and punctuation are ignored; words in the same order
  score higher.
- `search --goal-at FILE:LINE:COL [--limit N]`: lemmas about the goal at a position. The goal's
  target words are the query. Starts the Lean language server, and cannot be combined with a query.
- `index scan` (or `refresh`): update the index at `.tpt-lean/index.sqlite`. Only changed files are
  read again.
- `index status`: schema version, file, symbol and import counts, and an integrity check.
- `index clear`: delete the index.
- `index sidecar [--exe PATH] [--timeout S]`: run the Lean sidecar over the project's modules, and
  store declaration types and docstrings. The sidecar is built with `lake -R build` in
  `lean/tpt-lean-sidecar`, or taken from `TPT_LEAN_SIDECAR`. It must be built for the Lean version in
  use.

## Lean

These commands run Lean or Lake.

- `build [target] [--json] [--watch] [--poll-ms N] [--explain-slow] [--report] [--plan] [--timeout S]`: runs
  `lake build` and reports its progress as events. `--json` prints one event per line, as Lake
  reports them: `build_started`, `cache_hit` or `cache_miss`, `file_started`, `file_finished`,
  `file_failed`, `diagnostic`, `build_finished`. `--watch` rebuilds when the sources change, polling
  every `--poll-ms` milliseconds (default 1000); it is not an operating-system file watcher.
  `--explain-slow` lists the slowest files. `--plan` shows the import waves of the project without
  running Lake. A cache hit means a successful build with the same inputs is recorded; Lake still
  runs.
  `--report` prints where the time went: whether a cache layer (`memory`, `disk` or `shared`) served
  the build, the time to compute the cache key, the rest of the total (Lake and the compiler), and the
  ten slowest files. It is built from the same events as the other flags; nothing runs twice.
- `watch [--once] [--interval-ms N] [--timeout S]`: checks every `.lean` file once, then checks a file
  again when its length or modification time changes. Each file gets one summary line (errors,
  warnings, `sorry` count), and the first three errors. `lakefile.lean` is not checked, because only
  Lake reads it. `--once` checks everything and exits 1 if any file has an error, which is useful in
  CI. Without `--once` it runs until interrupted. With `--json`, each summary is one envelope line.
  Polling checks file stamps, not contents, and does not run Lake.
- `check [file] [--timeout S]`: `lake build` for the project, or `lean <file>` for one file. The
  diagnostics are saved for `explain --last-error`.
- `goals FILE:LINE:COL [--timeout S]`: the plain goal at a position, from the Lean language server.
  `goals FILE:LINE:COL --sidecar [--exe PATH]` computes the goals with the Lean sidecar instead: the
  file is elaborated by Lean's own frontend and the goals come from the tactic info trees. No language
  server is started. The sidecar must be built for the Lean version in use (see
  [install.md](install.md)); `--exe` or `TPT_LEAN_SIDECAR` names it. The output has `target`, which
  says what the goals are about: `tactic` (the goals before the innermost tactic containing the
  position), `term` (the expected type of the innermost term, with the hypotheses in scope, as in
  `theorem t : P := sorry`), `statement` (the type of the declaration whose name is at the position),
  or `none`. The sidecar's `sites` mode answers several positions of one file in one elaboration.
  Language-server goals have no `target`.
- `explain FILE:LINE:COL [--timeout S]`: the error at a position, the goal, and lemma candidates from
  the index when one exists. `explain --last-error`: the diagnostics saved by the last `check`.
- `context-pack FILE:LINE:COL [--max-tokens N] [--timeout S]`: a bundle for an AI agent: a source
  snippet, the goal, hypotheses, diagnostics, imports, nearby declarations, and lemma candidates. The
  snippet, goal and diagnostics are never dropped, so a very large diagnostic can exceed the budget.
  Each call starts Lean; a running `server` keeps one Lean session for `context.pack`.
- `profile FILE [--slowest N] [--json] [--imports] [--graph OUT.dot] [--graph-depth N]
  [--flamegraph OUT.folded] [--timeout S]`: the wall time, the peak memory of the Lean process tree
  (sampled), the time per declaration from Lean's trace, tactic totals, and suggested actions.
  `--imports` times each direct import, with one `lean` run per import. `profile --build [--timeout S]`
  runs the project build and prints the same report as `build --report` (cache outcome, time per step,
  slowest files). `--graph` writes a Graphviz
  graph of the imports; `--graph-depth` (1 to 4) expands project modules too. `--flamegraph` writes
  folded stacks for `flamegraph.pl` or `inferno`. `profile --compare A.json B.json` shows the
  differences between two saved reports.
- `docgen [--out DIR] [--open] [--json] [--include-deps]`: a static HTML site (default
  `.tpt-lean/docs`) with declaration pages, a namespace tree, source links, dark mode, a search index,
  and an import graph. The site needs an HTTP server for its search.
  `docgen --from-sidecar Module.Name [--exe PATH] [--timeout S]` documents one built module from the
  Lean sidecar instead of the source parser: names, kinds, pretty types and docstrings come from
  Lean's own elaborated environment. The exe is `--exe` or `TPT_LEAN_SIDECAR`, as for `index sidecar`.
  The module must be built (`lake build`). The sidecar reports no imports or source lines, so the
  site shows no import graph for that module, its declarations have no line numbers, and the source
  page is shown only when the module's `.lean` file exists. `--include-deps` is refused with it.
- `ci [--json] [--sarif] [--timeout S]`: builds the project, collects the diagnostics, and reports
  the result. `--json` prints the CI report; `--sarif` prints a SARIF 2.1.0 log for code-scanning
  tools. Exit 1 when the build failed. `scripts/validate-sarif.py` checks the SARIF output against
  the official schema.
- `bisect [--axis commits|imports|flags] [--timeout S]`: finds the first failing candidate with a
  `lake build` check per probe. Any other `--axis` value is an invalid argument (exit 2).
  - `--axis commits` (default): `--good REV --bad REV`. The first failing commit between two
    revisions. Each step runs `lake build` in a temporary worktree.
  - `--axis imports --file FILE`: the imports of a project file, in file order. Probe `k` keeps the
    first `k` imports and removes the rest, then runs `lake build <module of FILE>`. The first
    failing import is the last one of the smallest failing prefix. The state with no imports is
    assumed to pass.
  - `--axis flags --file FILE --flags "a=1,b,c=false"`: Lean options in the order given. A bare name
    means `true`. Names may contain letters, digits, `.` and `_`; values may also contain `-`. Probe
    `k` writes the first `k` as `set_option` lines after the last import, then builds. The state
    with no options is assumed to pass.
  - `imports` and `flags` rewrite FILE for each probe and restore the original text afterwards (also
    on error). Do not edit FILE while the bisect runs. The probes run `lake build` once each, so
    the search costs about `log2(n) + 1` builds.
  - The output is an `ItemBisectReport` (see [json-schemas.md](json-schemas.md)). Dependency versions
    and Mathlib updates are not bisected; they are listed in `not_done`.

## Servers and integrations

- `server start [--port N] [--http PORT] [--project DIR]`: run the JSON-RPC API in the foreground.
  On Windows the default transport is a named pipe; on Linux and macOS it is a Unix socket in
  `.tpt-lean/`. `--port N` uses loopback TCP instead (0 picks a free port). `--http PORT` also serves
  `POST /v1/rpc` and `GET /v1/events` on `127.0.0.1`. The server records its state in
  `.tpt-lean/server.json`. Methods and permissions: [server-api.md](server-api.md).
- `server status` and `server stop`: read the state file and probe or stop the server.
- `mcp stdio`: the MCP server over stdin and stdout. Tools are classed read, write, build or profile;
  a denied tool returns an error that names the missing permission. See [mcp.md](mcp.md).
- `mcp http [--port N]`: the MCP server over HTTP on `127.0.0.1` (default port 7474).
- `lsp`: the Lean language-server proxy over stdin and stdout, for editors. It answers document and
  workspace symbols, hover goals, go-to-definition for indexed names, code actions and formatting. See
  [lsp.md](lsp.md) and [editors.md](editors.md).

## Help

`tpt-lean help` (or `tpt-lean` with no arguments) lists the commands. An unknown command exits with
code 2.
