# CLI reference

Binaries: `tpt-lean` and the short alias `tlean`.

## Global flags

| Flag | Meaning |
| --- | --- |
| `--json` | print one JSON envelope (`schema_version`, `tool`, `kind`, `ok`, `data`) |
| `--ndjson` | print one JSON record per line for list results |
| `--project <dir>` | project root; default: nearest `tpt-lean.toml` or `lakefile.*` above the working directory |
| `--set section.key=value` | configuration override (highest precedence), repeatable |
| `-v`, `--verbose` | debug logging to stderr (or set `TPT_LEAN_LOG`) |
| `-q`, `--quiet` | errors only |

## Positions

`FILE:LINE:COL` uses a 1-based line and a 1-based column that counts characters. Lean
positions are converted to UTF-16 units internally for the language server.

## Configuration precedence

defaults < `tpt-lean.toml` < `.tpt-lean/config.toml` < environment (`TPT_LEAN_<SECTION>_<KEY>`) <
`--set`. See `tpt_lean_core::config` for the list of recognised environment variables.

## JSON errors

On failure with `--json`, stdout contains:

```json
{"schema_version":1,"tool":"tpt-lean","kind":"error","ok":false,"error":{"code":"timeout","message":"...","path":null}}
```

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | success |
| 1 | a check failed (`fmt --check` would change files, or Lean reported errors) |
| 2 | invalid argument or unknown flag |
| 3 | Lean/Lake or project not found |
| 4 | configuration invalid |
| 5 | path outside the workspace |
| 6 | index error |
| 7 | syntax invalid or uncertain (formatter refused) |
| 8 | timeout or output limit exceeded |
| 9 | I/O error |
| 10 | not implemented yet |
| 70 | internal error |

## Commands

Environment and project:
- `init <dir> [--mathlib] [--template default|theorem|teaching|library]`
- `doctor`: toolchain, lean-version, project, config, index, cache, server socket, language server start and shutdown, cache age. `lean-version` warns when the Lean on PATH differs from the `lean-toolchain` pin, or is not one of the versions this release is tested against (currently 4.36.0). `explain` prints a warning when that version is untested, and its JSON carries the same `lean` compatibility object the server returns. Warnings include a suggestion where one applies.
- `config show`
- `clean [--timeout S]`: runs `lake clean`
- `cache status`
- `cache prune [--days N] [--dry-run]`: removes old files in the global cache only (build records and tool memos included)
- `cache export DIR` / `cache import DIR`: copy build records to or from DIR, for CI artifacts. `cache status` lists the shared layers named by `TPT_LEAN_SHARED_CACHE`.

Source:
- `fmt [--check] [--diff] [--verify] [paths...]` (`--verify` needs Lean and is not implemented yet)
- `imports [file]`: lists `import` lines for one file or the whole project
- `imports --unused FILE`: heuristic. An import is flagged when none of the module's declared names appear in the file. Modules with instances, notations, tactics, or attributes are never flagged.
- `imports --suggest "QUERY" [--limit N]`: modules whose declarations match the query (needs an index)
- `imports --minimize FILE`: prints the proposed removal as a diff. The file is never written.
- `imports --cost`: direct import counts per module (structural estimate, not compile time)
- `search <query> [--namespace NS] [--near TERM]... [--limit N]`, or `search --goal-at FILE:LINE:COL` to search for lemmas about the goal at a position (the goal's target words are the query; it starts the Lean language server, and it cannot be combined with a query)
- `search <query> --from FILE`: symbols from modules that FILE imports rank higher.
- `search --type PATTERN`: declarations whose type contains every identifier in PATTERN (for example `Nat → Nat` or `List Nat`), as whole tokens. Types come from `tpt-lean index sidecar`, so this needs Lean. Arrows and punctuation are ignored; token order raises the score.
- `index scan | refresh | status | clear`

Lean (needs `lean` and `lake` on PATH or in `~/.elan/bin`):
- `build [target] [--json] [--watch] [--poll-ms N] [--explain-slow] [--timeout S]`: runs `lake build`. `--json` prints one event per line (`build_started`, `cache_hit`/`cache_miss`, `file_started`, `file_finished`, `file_failed`, `diagnostic`, `build_finished`). Lake's output is parsed after it exits, so events are replayed rather than streamed. `--watch` polls for changes. It is not an OS file watcher.
- `check [file] [--timeout S]`: runs `lake build`, or `lean <file>`. Diagnostics are saved for `explain --last-error`.
- `goals FILE:LINE:COL`: plain goal at a position, from the Lean language server
- `explain FILE:LINE:COL`: error at a position plus the goal, and lemma candidates from the project index when one exists
- `explain --last-error`: diagnostics saved by the last `check`
- `context-pack FILE:LINE:COL [--max-tokens N]`: bundle for AI agents: snippet, goal, hypotheses, diagnostics, imports, nearby declarations, and index lemma candidates. The snippet, goal, and diagnostics are never dropped, so the budget can be exceeded for very large diagnostics.
- `profile FILE [--slowest N] [--json] [--imports] [--graph OUT.dot] [--graph-depth N]`: wall time, peak memory of the Lean process tree (sampled), per-declaration time from Lean's trace, tactic totals, and suggested actions. Project imports resolve through `lake env` when the project is a Lake project. `--imports` also times each direct import (one `lean` run per import, minus an empty-file baseline; approximate, and it costs one run per import). `--graph OUT.dot` writes a Graphviz graph of the imports with their load costs, implies `--imports`, and draws the most expensive imports in red. `--graph-depth N` (1 to 4, default 1) also expands project modules: their own imports are timed and drawn beneath them, with at most 60 timed runs. The single most expensive import is marked as the bottleneck.
- `profile --compare A.json B.json`: differences between two saved reports
- `profile FILE --flamegraph OUT.folded`: writes folded stacks (`stack;stack weight`, weight = self time in microseconds) for flamegraph.pl or inferno. Render with `flamegraph.pl OUT.folded > out.svg`.
- `docgen [--out DIR] [--open] [--json] [--include-deps]`: static HTML site (default `.tpt-lean/docs`) with declaration pages, a namespace tree, source links, dark mode, a search index, and an import list. The Dependencies page draws the import graph between documented modules as inline SVG (omitted above 200 modules). Search needs the site served over HTTP.
- `ci [--json] [--sarif] [--timeout S]`: build, diagnostics, and failure summaries. Cache statistics come from the build cache: a hit means a successful build with the same key is already recorded. Lake still runs, and a passing run is recorded. SARIF is checked structurally, not against the official schema.
- `bisect --good REV --bad REV [--timeout S]`: first failing commit, with a `lake build` check per probe in a temporary worktree. Dependency versions, Mathlib, flags, and imports are not bisected.
- `lsp`: Lean language-server proxy over stdin/stdout (document symbols, workspace symbols from the index, hover goals, cached diagnostics)

Servers:
- `mcp stdio`: MCP over stdin/stdout. Tools are classed read, write, build, or profile, and denied tools return an error naming the missing permission. `tpt_lean_profile_file` needs `security.allow_build` and `security.allow_profile`.
- `mcp http [--port N]`: MCP over local HTTP on 127.0.0.1 (default 7474)
- `server start [--port N]`: JSON-RPC API on 127.0.0.1 (7373 by default; remote only with `server.allow_remote`). Writes `.tpt-lean/server.json`.
- `server status`, `server stop`: read and stop the server recorded in `.tpt-lean/server.json`

