# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

Initial development version `0.1.0`. Nothing has been published yet.

### Added
- Cargo workspace with crates for core config and discovery, syntax, indexing,
  search, formatting, the Lean CLI/LSP adapter, FFI, JSON-RPC server, MCP server,
  docgen, profiling, build orchestration, LSP proxy, WASM bindings, and the
  `tpt-lean` / `tlean` CLI.
- Lean integration: `lean` / `lake` invocation with typed options, diagnostic
  parsing, goals queries, and `check`, `goals`, `explain`, `context-pack`.
- `tpt-lean` CLI commands: `search`, `fmt`, `index`, `doctor`, `build`, `clean`,
  `cache`, `profile`, `docgen`, `imports`, `init`, `ci`, `server`, `mcp`, `lsp`.
- JSON and NDJSON output modes for every feature.
- C ABI (`tpt-lean-ffi`) with a hand-written header and ABI version query.
- Permission model for the server and MCP: read by default; write, build and
  profile are explicit opt-ins.

### Security
- No arbitrary shell execution: external commands are built from typed options.
- Path parameters are checked against workspace roots (including symlink escape,
  device files, and UNC/`\\?\` paths).
- FFI exports are wrapped in `catch_unwind`; no panics cross the boundary.

### Licensing
- Dual-licensed `MIT OR Apache-2.0`. Dependencies are limited to the allow list in
  `deny.toml` and checked by `cargo deny` and `cargo audit` in CI.
