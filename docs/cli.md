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
- `doctor`: checks Lean and Lake, the toolchain pin, the project configuration, the dependency
  cache, Mathlib, write permissions, the index, the cache, the server socket, and whether the
  language server starts and stops. A `lean-version` check warns when the Lean on `PATH` differs
  from the pin, or is not one of the tested versions (currently 4.35.0 and 4.36.0). Exits 3 when a
  check fails.
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
- `search <query> [--from FILE] [--namespace NS] [--near TERM]... [--limit N]`: names and
  docstrings in the index. `--from FILE` ranks symbols from modules that the file imports higher.
  `--near` keeps only files that also declare a name matching each term.
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

- `build [target] [--json] [--watch] [--poll-ms N] [--explain-slow] [--plan] [--timeout S]`: runs
  `lake build` and reports its progress as events. `--json` prints one event per line, as Lake
  reports them: `build_started`, `cache_hit` or `cache_miss`, `file_started`, `file_finished`,
  `file_failed`, `diagnostic`, `build_finished`. `--watch` rebuilds when the sources change, polling
  every `--poll-ms` milliseconds (default 1000); it is not an operating-system file watcher.
  `--explain-slow` lists the slowest files. `--plan` shows the import waves of the project without
  running Lake. A cache hit means a successful build with the same inputs is recorded; Lake still
  runs.
- `check [file] [--timeout S]`: `lake build` for the project, or `lean <file>` for one file. The
  diagnostics are saved for `explain --last-error`.
- `goals FILE:LINE:COL [--timeout S]`: the plain goal at a position, from the Lean language server.
  `goals FILE:LINE:COL --sidecar [--exe PATH]` computes the goals with the Lean sidecar instead: the
  file is elaborated by Lean's own frontend and the goals come from the tactic info trees. No language
  server is started. The sidecar must be built for the Lean version in use (see
  [install.md](install.md)); `--exe` or `TPT_LEAN_SIDECAR` names it.
- `explain FILE:LINE:COL [--timeout S]`: the error at a position, the goal, and lemma candidates from
  the index when one exists. `explain --last-error`: the diagnostics saved by the last `check`.
- `context-pack FILE:LINE:COL [--max-tokens N] [--timeout S]`: a bundle for an AI agent: a source
  snippet, the goal, hypotheses, diagnostics, imports, nearby declarations, and lemma candidates. The
  snippet, goal and diagnostics are never dropped, so a very large diagnostic can exceed the budget.
  Each call starts Lean; a running `server` keeps one Lean session for `context.pack`.
- `profile FILE [--slowest N] [--json] [--imports] [--graph OUT.dot] [--graph-depth N]
  [--flamegraph OUT.folded] [--timeout S]`: the wall time, the peak memory of the Lean process tree
  (sampled), the time per declaration from Lean's trace, tactic totals, and suggested actions.
  `--imports` times each direct import, with one `lean` run per import. `--graph` writes a Graphviz
  graph of the imports; `--graph-depth` (1 to 4) expands project modules too. `--flamegraph` writes
  folded stacks for `flamegraph.pl` or `inferno`. `profile --compare A.json B.json` shows the
  differences between two saved reports.
- `docgen [--out DIR] [--open] [--json] [--include-deps]`: a static HTML site (default
  `.tpt-lean/docs`) with declaration pages, a namespace tree, source links, dark mode, a search index,
  and an import graph. The site needs an HTTP server for its search.
- `ci [--json] [--sarif] [--timeout S]`: builds the project, collects the diagnostics, and reports
  the result. `--json` prints the CI report; `--sarif` prints a SARIF 2.1.0 log for code-scanning
  tools. Exit 1 when the build failed. `scripts/validate-sarif.py` checks the SARIF output against
  the official schema.
- `bisect --good REV --bad REV [--timeout S]`: the first failing commit between two revisions. Each
  step runs `lake build` in a temporary worktree. Dependency versions, Mathlib updates, flags and
  imports are not bisected.

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
