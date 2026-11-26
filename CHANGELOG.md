# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

Initial development version `0.1.0`. Nothing has been published yet.

### Added
- File scan honours `.gitignore` files in the project and its subdirectories (`tpt-lean-index::ignore`).
- `docs/security.md`, `docs/versioning.md`, `docs/editors.md`, and a Claude Desktop setup in `docs/mcp.md`.
- `tpt-lean init --template mathlib` as an alias for `--mathlib`.
- `tpt-lean init` no longer leaves an empty directory behind when the template name is unknown.
- `tpt-lean search --type PATTERN`: type-shaped search over sidecar declaration types.
- `tpt-lean search --from FILE`: symbols from modules that FILE imports rank higher.
- Server transport: Unix domain socket by default on Linux and macOS (owner-only, 0600), with
  `--port` for loopback TCP. `server start` now records its state, so `status` and `stop` work
  with either transport.
- LSP proxy answers `textDocument/definition` from the project index when the word under the
  cursor names a declaration exactly; otherwise it forwards to Lean.
- MCP resources `tpt-lean://profiles` and `tpt-lean://profile/{id}` for stored profile reports.
- `tpt-lean build --plan [--json]`: the project's module import waves, critical path and widest wave.
- LSP proxy code actions: an `Import M` quick fix for unknown identifiers, and the commands
  `tpt-lean.searchLemmas` and `tpt-lean.explainError` through `workspace/executeCommand`.
- LSP `textDocument/formatting` and a `Format document` action, using `tpt-lean fmt`.
- `tpt-lean build` prints the cache key time and whether the build was a cache hit, and the JSON
  output carries `key_ms`.
- Measurements (not tests): search p50 0.99 ms, p95 4.0 ms on 5,000 symbols; `fmt` median 76 ms on
  a 6,255-line file. Run them with `cargo test -p tpt-lean-search --release --test bench -- --ignored`
  and `cargo test -p tpt-lean-fmt --release --test bench -- --ignored`.
- Server: a warm Lean session per project for `context.pack` and `diagnostics.explain`, reused
  while requests keep arriving, re-elaborated on edits, and stopped after 10 minutes idle or at
  shutdown. Measured through the server: warm repeated text p50 1 ms; after an edit p50 231 ms,
  p95 234 ms; the first call (starts Lean) 4.7-6.0 s.
- Measured: incremental index refresh after one edit in a 500-file project, p50 27 ms (full refresh 372 ms).
- Server: optional loopback HTTP transport, `tpt-lean server start --http PORT`, serving
  `POST /v1/rpc` with a Host check that refuses non-loopback names.
- `tpt-lean build` reports Lake's progress as it runs: file and diagnostic events are emitted live
  (`tpt-lean-adapter::run_streaming` passes output lines to the build as they are read).
- Shared cache layer: `TPT_LEAN_SHARED_CACHE` names directories of build records (a team share).
  Lookups go memory, local disk, then shared; shared hits match on the digest and are copied locally.
  `tpt-lean cache export DIR` and `cache import DIR` move records for CI artifacts.
- Lean 4.35.0 (the previous stable release) is tested alongside 4.36.0: the full suite passes on both
  locally. The sidecar builds per version (`lake -R build -K buildDirName=.lake/build-v4.35.0`), and
  docstrings are trimmed because 4.35 keeps a trailing space that 4.36 drops. CI gained a
  `lean-integration` job for both versions (not run on GitHub).
- Go binding tests without cgo (`crates/tpt-lean-ffi/tests/go`): six tests load the Windows DLL with
  `syscall.LoadDLL` and pass against the debug build.
- Fixed: Lake 5 prints Lean's errors as `error: FILE:LINE:COL: message`, which the diagnostic parser
  did not read, so `build` and `ci` reported a failed build with no diagnostics. Both orders parse now.
- SARIF validation: `scripts/validate-sarif.py` checks `ci --sarif` output against the OASIS schema.
  A project with a Lean error produces two results, and both files validate.
- `init --mathlib` pins Mathlib to the tag v4.36.0 and sets `lean-toolchain` to match. The tag exists
  on the Mathlib repository. A full Mathlib build was not run.
- Named pipes on Windows (`crates/tpt-lean-pipe`): the API server listens on `\.\pipe	pt-lean-<hash>`
  by default there, with the default security descriptor and a refusal of a second server. Tested on
  Windows: connect, exchange, exclusive bind, clean end of a reply.
- `build.subscribe` over JSON-RPC: the build's events as `build.event` notifications, then
  `build.finished`, on any transport.
- Fixed: SSE and HTTP responses could be sent with bare line feeds where CRLF belongs; tests now check
  the line endings.
- npm package for the WebAssembly build (`crates/tpt-lean-wasm/npm`): `npm run build` (with the matching
  wasm-bindgen-cli 0.2.126) and `npm run example` both run here; `npm pack --dry-run` lists 7 files. The
  package stays private. A browser example is not written.
- CI `benchmarks` job: search, formatter and index benchmarks in release mode, results uploaded as an
  artifact. Advisory, so a missed target is reported without failing the build. Not run on GitHub yet.
- Known: a full `tpt-lean build` of a 41-module project is about 2.5 times slower than `lake build`
  from the same shell: 17-20 s against about 7 s in five interleaved runs. Measured and ruled out as
  causes: Lean's thread count, environment variables, stdio handling, process creation flags, the
  elan proxy (the real Lake binary is as slow from a non-shell parent), and CPU affinity and priority.
  The slowdown follows the direct parent of Lake: a shell (bash, PowerShell) or direct launch is
  fast; Rust, Python and cmd parents are slow. Concurrent Lean processes average about half as many
  in the slow case. The mechanism is not yet known.
- Known: a cache-hit `tpt-lean build` took about 2.6 s on a small project, against about 1.1 s for a
  direct `lake build` that does nothing. The cache key is about 1 ms. The extra time is in the Lake
  process that tpt-lean starts (about 2.4 s when started from Rust, 1.1 s from a shell); the cause
  is not yet known.
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
