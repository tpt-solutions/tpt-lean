# tpt-lean

A fast, Rust-powered toolbox for Lean 4. tpt-lean makes the workflow around Lean easier and
faster: project health checks, a formatter, a local symbol index and search, and a controlled
bridge to the official `lean` and `lake` tools.

**Lean remains the trusted proof checker.** tpt-lean does not reimplement Lean's kernel,
elaborator, or Mathlib. Index and cache results are accelerators, never sources of truth.

Owner: TPT Solutions

## Status

Early development. Every command below is implemented. Each one accepts `--json` (one envelope
object) or `--ndjson` (one record per line). The binaries are `tpt-lean` and `tlean`.
[docs/cli.md](docs/cli.md) lists every flag and exit code.

| Command | What it does |
| --- | --- |
| `init <dir> [--mathlib] [--template NAME]` | create a starter Lean project |
| `doctor [--fix]` | check Lean, Lake, project, config, index, and cache; `--fix` rebuilds a missing or damaged index |
| `config show` | print the effective configuration |
| `fmt [--check\|--diff\|--verify] [paths]` | conservative, idempotent formatter |
| `imports [file] [--unused\|--cost\|--minimize FILE\|--suggest QUERY]` | import analysis |
| `search <query> [--from FILE] [--namespace NS] [--near TERM] [--limit N]` | name, prefix, and docstring search; `--type` and `--goal-at` also |
| `index scan\|refresh\|status\|clear\|sidecar` | SQLite symbol index (incremental) |
| `build [target] [--watch\|--plan\|--report\|--explain-slow]` | `lake build` through typed options |
| `check [file]` | `lake build`, or `lean <file>` |
| `watch [--once] [--interval-ms N]` | re-check changed files; one diagnostics and `sorry` summary per file |
| `sorry-report [paths] [--deny-sorry]` | `sorry` and `admit` per declaration; `--deny-sorry` exits 1 when any is found |
| `goals FILE:LINE:COL` | goals at a position, from Lean's own info trees |
| `explain FILE:LINE:COL \| --last-error` | explain an error at a position |
| `context-pack FILE:LINE:COL` | bounded context for a proof, for an assistant or a person |
| `profile FILE [--imports] [--graph\|--flamegraph\|--compare]` | where Lean spends its time |
| `docgen [--out DIR]` | documentation from the sources |
| `ci [--json\|--sarif]` | run the project checks as one CI job |
| `bisect --good REV --bad REV` | first failing git commit |
| `clean` | `lake clean` |
| `upgrade --to VERSION [--apply]` | plan a `lean-toolchain` change and the Mathlib pin to update; `--apply` runs `lake build` and restores the file if it fails |
| `cache status\|prune\|export DIR\|import DIR` | the global build cache |
| `server start\|status\|stop` | the local JSON-RPC API server |
| `mcp stdio\|http` | the Model Context Protocol server |
| `lsp` | the language-server proxy for editors |
| `setup [--editor NAME]... [--mcp NAME]... [--dry-run]` | write editor and MCP config for the project, showing a diff first |
| `completions bash\|zsh\|fish\|powershell` | print a shell completion script |

Lean and Lake are needed only by the commands that run them (`build`, `check`, `goals`, `explain`,
`context-pack`, `profile`, `ci`, `bisect`, `watch`, `index sidecar`, `init --mathlib`, `upgrade
--apply`, `lsp`). See
[docs/install.md](docs/install.md).

## Build

Requires a stable Rust toolchain (1.82 or newer).

```
cargo build --workspace
cargo test --workspace
```

## Documentation

- [Install](docs/install.md): what you need, and how to build the `tpt-lean` binary
- [Quickstart](docs/quickstart.md): a new project to search, format and build, in a few commands
- [CLI reference](docs/cli.md): every command, flag and exit code
- [JSON output](docs/json-schemas.md): the envelope, the error codes, and the fields of each result
- [Editors](docs/editors.md), [MCP](docs/mcp.md), [API server](docs/server-api.md): integrations
- [Security](docs/security.md) and [versioning](docs/versioning.md)
- [Publishing](PUBLISHING.md): crate release order, what blocks each crate, and the release log

## Layout

```
crates/tpt-lean-core       configuration, errors, paths, hashing, process policy, JSON envelope
crates/tpt-lean-syntax     lossless lexer, shallow CST, symbols, imports, tactic blocks
crates/tpt-lean-index      SQLite index (WAL, FTS5)
crates/tpt-lean-search     ranking over the index
crates/tpt-lean-fmt        conservative formatter
crates/tpt-lean-adapter    typed invocations of lean/lake, LSP client
crates/tpt-lean-build      Lake build orchestration, event stream, build cache
crates/tpt-lean-profile    timing reports from Lean's trace output
crates/tpt-lean-docgen     static HTML documentation generator
crates/tpt-lean-tools      import analysis, bisect, CI and SARIF, doctor checks
crates/tpt-lean-lsp        Lean language-server proxy
crates/tpt-lean-mcp        Model Context Protocol server
crates/tpt-lean-server     local JSON-RPC API server
crates/tpt-lean-pipe       Windows named-pipe transport for the API server (not published)
crates/tpt-lean-ffi        stable C ABI (cdylib and staticlib)
crates/tpt-lean-wasm       browser-safe WebAssembly bindings (not published)
crates/tpt-lean-cli        the tpt-lean and tlean binaries
```

Each crate has its own README and CHANGELOG.

## Licensing

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
Unless you state otherwise, any contribution you submit is dual-licensed the same way.
Dependencies must be MIT, Apache-2.0 (as an alternative in an OR expression), BSD, ISC, Zlib,
Unlicense, CC0, or Unicode-3.0. See `docs/dependencies.md` and `deny.toml`.
