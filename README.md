# tpt-lean

A fast, Rust-powered toolbox for Lean 4. tpt-lean makes the workflow around Lean easier and
faster: project health checks, a formatter, a local symbol index and search, and a controlled
bridge to the official `lean` and `lake` tools.

**Lean remains the trusted proof checker.** tpt-lean does not reimplement Lean's kernel,
elaborator, or Mathlib. Index and cache results are accelerators, never sources of truth.

Owner: TPT Solutions

## Status

Early development. Implemented today:

| Command | What it does |
| --- | --- |
| `tpt-lean init <dir> [--mathlib]` | create a starter Lean project |
| `tpt-lean doctor` | check Lean, Lake, project, config, index, and cache |
| `tpt-lean config show` | print the effective configuration |
| `tpt-lean fmt [--check\|--diff] [paths]` | conservative, idempotent formatter |
| `tpt-lean index scan\|refresh\|status\|clear` | SQLite symbol index (incremental) |
| `tpt-lean search <query> [--namespace NS] [--near TERM] [--limit N]` | name, prefix, and docstring search |
| `tpt-lean build [target]` | `lake build` through typed options |
| `tpt-lean check [file]` | `lake build`, or `lean <file>` |
| `tpt-lean cache status` | global cache location and size |

Planned commands (goals, explain, context-pack, profile, MCP, server, FFI, docgen, LSP proxy)
exit with code 10 (`not_implemented`). See `todo.md` for the full plan.

Every command accepts `--json` (one envelope object) or `--ndjson` (one record per line).
The binaries are `tpt-lean` and `tlean`. See `docs/cli.md`.

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
