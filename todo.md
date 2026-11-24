# tpt-lean — Project Todo

Owner: TPT Solutions · License: MIT OR Apache-2.0 · Language: Rust
Core rule: Lean remains the trusted proof checker. tpt-lean accelerates the workflow around Lean.
Second rule: every advanced feature has a simple JSON output mode.

## Decisions (locked)
- [x] Project license: `MIT OR Apache-2.0` (spec said MIT only; changed)
- [x] Copyright holder: TPT Solutions
- [x] Allowed dependency licenses: MIT, BSD-2-Clause, BSD-3-Clause, ISC, Zlib, Unlicense, CC0-1.0, Unicode-3.0
- [x] Rejected: Apache-2.0-only, GPL, LGPL, AGPL, MPL, proprietary, unknown/custom
- [x] Platforms: Windows, Linux, macOS equally
- [x] MCP hand-written over serde_json (no `rmcp` unless its license is verified)
- [x] CI: GitHub Actions
- [x] Lean test targets: current stable + previous stable (nightly optional)
- [x] Lean sidecar in scope (Phase 2)
- [x] No prebuilt Mathlib indexes shipped; users index locally

---

## Phase 0 — Repository foundation
Crates: `tpt-lean-core`, `tpt-lean-cli`

### Repo and licensing
- [x] Initialize git repo, `.gitignore`, `.gitattributes` (line endings for Windows)
- [x] Cargo workspace `Cargo.toml` (shared package metadata, edition, MSRV, `license = "MIT OR Apache-2.0"`, authors/owner TPT Solutions)
- [x] `LICENSE-MIT` (copyright TPT Solutions)
- [x] `LICENSE-APACHE` (Apache-2.0 text, copyright TPT Solutions)
- [x] SPDX header convention (`SPDX-License-Identifier: MIT OR Apache-2.0`) and check
- [x] `README.md` (identity, license section, contribution = dual-licensed inbound)
- [x] `CONTRIBUTING.md` (issues-only; `SECURITY.md` removed)
- [x] `rust-toolchain.toml`, `rustfmt.toml`, clippy config

### Dependency policy
- [x] `deny.toml` (allow list above, `confidence-threshold = 0.95`, bans, sources)
- [x] Verify cargo-deny accepts `MIT OR Apache-2.0` crates and rejects an Apache-only test crate
- [x] `cargo deny check licenses` / `bans` / `advisories` locally
- [x] `cargo audit` locally
- [x] Script to generate dependency license report (used per release)
- [x] Document dependency policy (`docs/dependencies.md`)

### CI
- [ ] GitHub Actions: build + test on Windows, Linux, macOS
- [x] CI: `cargo fmt --check`, `cargo clippy -D warnings`
- [x] CI: cargo-deny (licenses, bans) and cargo-audit
- [x] CI: cache cargo registry/target
- [x] CI: dependency license report artifact

### tpt-lean-core
- [x] Error types and stable error codes
- [x] Config model + loading (`tpt-lean.toml`: project, build, index, server, security)
- [x] Config precedence (defaults, project, `.tpt-lean/config.toml`, env, CLI flags)
- [x] Project discovery (find `lakefile.lean` / `lakefile.toml` / `tpt-lean.toml`)
- [x] Toolchain detection (`lean`, `lake`, `elan`, `lean-toolchain`)
- [x] Path normalization (Windows/Unix, case, UNC, `\\?\`)
- [x] File identity and hashing (pick MIT-compatible hash crate, e.g. blake3 — check license)
- [x] Cache key types
- [x] Process execution policy types (timeout, memory limit, cancellation token, output limit, env sanitization)
- [x] Logging/tracing integration
- [x] JSON schema types and schema conventions (versioned, stable field names)
- [x] Data directory layout: `.tpt-lean/` (config, index.sqlite, logs, profiles, runs, tmp)
- [x] Global cache dir per OS (XDG / `~/Library/Caches` / `%LOCALAPPDATA%`)

### tpt-lean-cli
- [x] CLI shell with subcommand skeleton (all commands from spec section 15 stubbed)
- [x] Global flags: `--json`, `--ndjson`, `--project`, `--verbose`, `--quiet`
- [x] Binary names `tpt-lean` and alias `tlean`
- [x] Exit code conventions
- [x] Human vs JSON output abstraction
- [x] Unit tests for config loading, path normalization, cache keys

---

## Phase 1 — Immediate useful tools
Crates: `tpt-lean-syntax`, `tpt-lean-index`, `tpt-lean-search`, `tpt-lean-fmt`

### tpt-lean-syntax
- [x] Tokenizer (keywords, identifiers with namespaces/guillemets, numbers, strings, unicode ops, comments incl. nested `/- -/` and doc comments)
- [x] Source spans / `TextRange`, line/column mapping (UTF-8 and UTF-16 for LSP)
- [x] Lossless CST (`SyntaxNode`, `SyntaxElement`, `Token`)
- [x] Comment and whitespace preservation (round-trip test: parse → print == input)
- [x] Indentation tracking
- [x] Import extraction
- [x] Declaration boundary detection (theorem, def, lemma, instance, structure, inductive, class, abbrev, namespace, section, etc.)
- [ ] Tactic block detection (`by`, `·`, `case`, etc.)
- [x] Lightweight symbol extraction (name, kind, namespace, range, docstring)
- [x] Error recovery and "uncertain" markers (needed by formatter safety)
- [x] Fuzz/property tests (no panics, round-trip)
- [ ] Golden tests: `input.lean`, `expected_symbols.json`

### tpt-lean-index
- [x] SQLite integration (choose crate, verify license, bundled SQLite)
- [ ] Schema + migrations: projects, files, symbols, docstrings, imports, dependencies, diagnostics, build_runs, profile_runs, cache_entries
- [x] FTS5 setup for name/docstring search
- [ ] Fast file scan (respect ignore rules, `.lake` handling)
- [x] Syntax-level indexing using `tpt-lean-syntax`
- [x] Import graph / dependency edges
- [ ] Incremental invalidation by file hash and import graph
- [x] Mathlib/dependency indexing from local `.lake/packages` (no redistribution)
- [x] Index health check API (used by doctor)
- [ ] Schema version + upgrade path
- [x] `tpt-lean index scan` / `refresh` / `status`
- [x] Concurrency model (readers while writer refreshes; WAL)
- [ ] Hooks for semantic enrichment (filled in Phase 2)

### tpt-lean-search
- [ ] Name search (exact, prefix, namespace-aware)
- [ ] Docstring search (FTS5)
- [x] `--namespace` filter and `--near` mode
- [ ] Ranking factors (exact name, namespace proximity, docstring relevance, imported modules first, kind, dependency distance; recency later)
- [x] Output: table, JSON, NDJSON
- [x] `tpt-lean search` CLI wiring
- [ ] Benchmarks vs targets (warm p50 < 10 ms, p95 < 50 ms)
- [ ] Type-shaped search placeholder (completed in Phase 2)

### tpt-lean-fmt
- [x] Trailing whitespace and final newline normalization
- [x] Indentation normalization
- [x] Simple tactic block alignment
- [x] Import formatting (sort/dedupe, conservative)
- [x] Spacing around common tokens
- [x] Comments preserved exactly
- [x] Refuse to write if syntax tree invalid or uncertain
- [x] Idempotency tests (fmt(fmt(x)) == fmt(x))
- [x] `tpt-lean fmt`, `fmt --check`, `fmt <file>`, `fmt --diff`
- [ ] `fmt --verify` stub (needs Lean; completed in Phase 2)
- [ ] Golden tests: `input.lean` → `expected_formatted.lean`
- [ ] Benchmark: 5,000-line file < 100 ms

### doctor (initial)
- [x] `tpt-lean doctor`: Lean, Lake, toolchain compat, project config, dependency cache, Mathlib availability, write permissions, SQLite index health
- [ ] Doctor suggestions (e.g. `cache prune`)
- [x] Doctor JSON output
- [ ] Server socket health (stub until Phase 3)

---

## Phase 2 — Lean semantic adapter
Crate: `tpt-lean-adapter`

### Level 1 — CLI integration
- [x] `lean --version`, `lake --version`, `lake env printenv`
- [x] Controlled `lake build` (typed options only)
- [x] Controlled `lean` invocation (typed options only)
- [x] Output parsing into structured diagnostics (severity, file, line, col, message)
- [ ] Process runner enforcing timeout, memory limit (where supported), cancellation, output size limit, env sanitization
- [ ] Windows process handling (job objects for kill/limit) and POSIX equivalents
- [x] No arbitrary shell execution anywhere (test for it)

### Level 2 — LSP integration
- [ ] JSON-RPC LSP client over stdio to `lake serve` / `lean --server`
- [ ] Document sync, diagnostics
- [ ] Goals query (`$/lean/plainGoal`, `plainTermGoal`)
- [ ] Document / workspace symbols
- [ ] Request cancellation, restarts, crash recovery
- [ ] Version compatibility handling across Lean versions

### Level 3 — Lean sidecar
- [ ] Small Lean package emitting structured JSON (symbols, types, goals) using Lean APIs
- [ ] Sidecar licensed MIT OR Apache-2.0 like the rest
- [ ] Sidecar build/versioning against current + previous stable Lean
- [ ] Semantic enrichment into index (declaration types, docstrings)

### Level 4 — Native artifact reader
- [ ] Deferred: evaluate `.olean`/`.ilean` stability; record decision (do not implement yet)

### Commands
- [x] `tpt-lean check` (file and project)
- [ ] `tpt-lean goals <file>:<line>:<col>`
- [ ] `tpt-lean explain <file>:<line>:<col>` and `--last-error` (JSON schema per spec 8.3)
- [ ] `tpt-lean context-pack <file>:<line>:<col> [--json] [--max-tokens]`
- [ ] Context pack content: snippet, goal, hypotheses, diagnostics, imported symbols, nearby defs, relevant lemmas, recent build errors, project constraints
- [ ] Token budgeting for context pack
- [ ] Type-shaped search (`--type`) and goal-based search (`--goal-at`)
- [ ] `fmt --verify`
- [ ] Cached diagnostics / safe cached goals in index
- [ ] Integration tests against current + previous stable Lean (nightly optional)
- [ ] Context pack perf target (p50 < 100 ms, p95 < 500 ms excl. elaboration)

---

## Phase 3 — FFI and API foundation
Crates: `tpt-lean-ffi`, `tpt-lean-server`

### tpt-lean-ffi
- [x] Crate setup (`cdylib` + `staticlib`)
- [x] Hand-written C header `tpt_lean.h` as ABI contract
- [x] `tptl_abi_version()` (separate from crate version)
- [x] Context new/free, `tptl_last_error`
- [x] Project discovery
- [x] Index open/free
- [x] Search query + results free + results to JSON
- [ ] Syntax parse
- [x] `tptl_format_text` + `tptl_string_free`
- [ ] Diagnostic parsing
- [ ] Cache query
- [x] Stable error codes (`TptlStatus`)
- [ ] No panic unwinding across boundary (`catch_unwind` on every export)
- [ ] No Rust types across boundary; explicit lengths; explicit ownership
- [x] Document thread safety per handle type
- [ ] Tests: C harness, Python ctypes, Go cgo
- [ ] FFI tests: memory frees, invalid args, error handling, ABI version, concurrent reads
- [ ] Windows/Linux/macOS build of shared lib
- [ ] Later (tracked, not MVP): build orchestration, profiling, goals, async callbacks, event streams
- [ ] Optional: header generation (not a blocker)

### tpt-lean-server
- [x] JSON-RPC 2.0 core (shared crate/module reused by MCP)
- [ ] Transports: Unix domain socket (Linux/macOS), named pipe or localhost TCP (Windows)
- [ ] Optional localhost HTTP: `POST /v1/rpc`
- [x] Remote access disabled by default
- [ ] Methods: `project.info`, `project.doctor`
- [ ] Methods: `search.query`, `search.symbol`, `search.goal`
- [ ] Methods: `index.status`, `index.refresh`
- [ ] Methods: `format.text`, `format.file`
- [ ] Methods: `check.file`, `check.project`
- [ ] Methods: `goals.at`, `diagnostics.get`, `diagnostics.explain`
- [ ] Methods: `imports.suggest`, `imports.unused` (wired in Phase 7)
- [ ] Methods: `context.pack`
- [ ] Methods: `build.run/cancel/status`, `profile.run/get` (wired in Phases 5–6)
- [ ] Streaming: `GET /v1/events` (SSE) or JSON-RPC notifications
- [ ] API request/response JSON schemas
- [ ] Permission enforcement per request (read/write/build/profile/network)
- [ ] Path safety checks on every path param
- [ ] `tpt-lean server start|stop|status`
- [ ] Persistent daemon behavior (warm index/LSP, idle shutdown)
- [ ] Schema-based request/response tests, permission tests

---

## Phase 4 — MCP server
Crate: `tpt-lean-mcp` (hand-written over serde_json)

- [ ] MCP protocol implementation (initialize, capabilities, tools/list, tools/call, resources, prompts, cancellation, progress)
- [ ] Verify target MCP protocol version(s)
- [x] stdio transport (first)
- [x] `tpt-lean mcp stdio`
- [x] Tool `tpt_lean_search`
- [ ] Tool `tpt_lean_context_pack`
- [ ] Tool `tpt_lean_goals_at`
- [x] Tool `tpt_lean_check_file`
- [x] Tool `tpt_lean_format_text`
- [ ] Tool `tpt_lean_explain_diagnostic`
- [ ] Tool `tpt_lean_format_file` (write permission)
- [ ] Tool `tpt_lean_build_project` (build permission)
- [ ] Tool `tpt_lean_profile_file` (profile permission)
- [ ] Tool `tpt_lean_imports_suggest`
- [x] Tool `tpt_lean_index_status`
- [ ] Tool input/output JSON schemas
- [ ] Resources: project info, diagnostics, index status, file symbols/ast/goals, build log, profile
- [ ] Optional prompts: explain_lean_error, find_lemma_for_goal, reduce_proof_timeout, suggest_imports, summarize_proof_state
- [ ] Local HTTP/SSE transport
- [ ] Permission model (default read-only, explicit write/build/profile)
- [ ] Tests: schema validation, permission enforcement, streaming cancellation, timeouts
- [ ] Client setup docs (Claude Code, Claude Desktop, other MCP clients)

---

## Phase 5 — Build/cache acceleration
Crate: `tpt-lean-build`

- [ ] Project understanding (module graph from imports)
- [ ] Safe Lake invocation via adapter
- [ ] Parallel orchestration where possible
- [ ] Cache key: lean version, package versions, lakefile hash, lock hash, source hash, import hashes, flags, env vars, target triple
- [ ] Cache layers: memory, local disk, optional shared team cache, optional CI cache
- [ ] Build event stream (`file_started`, `file_finished`, etc.) as structured events
- [ ] `tpt-lean build` (human progress)
- [ ] `tpt-lean build --json`
- [ ] `tpt-lean build --watch` (cross-platform file watcher; license check)
- [ ] `tpt-lean build --explain-slow`
- [x] `tpt-lean clean`
- [x] `tpt-lean cache status`
- [x] `tpt-lean cache prune`
- [ ] Cache-hit overhead < 200 ms
- [ ] Better logs and diagnostics
- [ ] Wire `build.*` into server and MCP
- [ ] Tests for cache correctness/invalidation
- [ ] Later: deeper build graph support (tracked, not initial)

---

## Phase 6 — Profiling, LSP proxy, docgen
Crates: `tpt-lean-profile`, `tpt-lean-lsp`, `tpt-lean-docgen`

### tpt-lean-profile
- [ ] Per-file timing
- [ ] Per-declaration timing
- [ ] Tactic timing where available
- [ ] Memory usage tracking
- [ ] Build-step timing, cache hit/miss reports
- [ ] Slow import detection
- [ ] Dependency bottleneck graph
- [ ] Flamegraph output
- [ ] JSON reports
- [ ] Suggested actions in report
- [ ] `tpt-lean profile <file>`, `--flamegraph`, `--slowest N`, `--compare a.json b.json`
- [ ] Wire into server/MCP

### tpt-lean-lsp
- [ ] Rust LSP proxy to official Lean LSP
- [ ] Faster document sync, cached symbols, cached diagnostics
- [ ] Instant project search
- [ ] Better progress reporting
- [ ] Request cancellation
- [ ] Lightweight syntax features before elaboration finishes
- [ ] Features: go-to-definition, hover, document/workspace symbols, diagnostics, goal display, proof-state panel support
- [ ] Code actions: import suggestions, lemma search, formatting, explain-error
- [ ] `tpt-lean lsp`
- [ ] Editor setup docs (VS Code, Neovim, Emacs, Helix)

### tpt-lean-docgen
- [ ] Input from indexed sources, sidecar JSON, LSP symbols
- [ ] Static HTML output
- [ ] Local search index
- [ ] Dark mode
- [ ] Source links
- [ ] Theorem/definition pages
- [ ] Namespace tree
- [ ] Dependency graph
- [ ] JSON export
- [ ] `tpt-lean docgen`, `--open`, `--json`, `--include-deps`

---

## Phase 7 — Remaining CLI utilities (not in spec phase list)
- [x] `tpt-lean init <name>` with generated lakefile.lean, lean-toolchain, Main.lean, tpt-lean.toml, .gitignore, README.md
- [ ] `init --mathlib`
- [ ] Templates: theorem, mathlib, teaching, library
- [x] `tpt-lean imports` (list graph)
- [ ] `imports --unused`
- [ ] `imports --suggest "<query>"`
- [ ] `imports --minimize <file>`
- [ ] Import dependency cost / heavy import detection / compile-time estimate
- [ ] `tpt-lean bisect --good --bad` (commits, dependency versions, Mathlib updates, flags, imports)
- [ ] `tpt-lean ci --json --sarif`
- [ ] SARIF output + schema validation
- [ ] Machine-readable failure summaries, cache stats in CI output
- [ ] Doctor extensions (server socket health, LSP health, cache age warnings)
- [ ] Security hardening pass (path safety fuzzing, symlink escape, device files, timeouts)

---

## Phase 8 — Optional WASM
Crate: `tpt-lean-wasm`

- [ ] Verify dependencies compile to WASM and licenses still pass
- [ ] Syntax tokenizer binding
- [ ] Formatter binding
- [ ] CST viewer
- [ ] Search over prebuilt compact index
- [ ] Documentation search
- [ ] Restricted browser-safe API (no process spawn, no native FS)
- [ ] npm packaging and browser/Node examples
- [ ] Tests (wasm-bindgen-test or equivalent)

---

## Phase 9 — Release, packaging, docs
- [ ] Verify performance targets (search, context pack, fmt, cache-hit build, incremental index)
- [ ] Benchmark suite in CI
- [ ] Per-release dependency license report
- [ ] Per-release `cargo deny` / `cargo audit` gate
- [ ] Versioning policy (crate semver vs ABI version)
- [ ] CHANGELOG
- [ ] User docs (install, quickstart, every command, JSON schemas)
- [ ] Security model docs (permission classes, path safety, command safety, resource limits)
- [ ] Integration docs: FFI, JSON-RPC API, MCP
- [ ] Generated client SDKs (decide languages)
- [ ] Decide: publish crates to crates.io (check `tpt-lean-*` name availability)
- [ ] Decide: prebuilt binaries on GitHub Releases (Windows/Linux/macOS, checksums)
- [ ] Decide: other packaging (winget, Homebrew, cargo install)
- [ ] Deferred: review attribution/NOTICE requirements before ever shipping Mathlib-derived prebuilt indexes
- [ ] Deferred: native `.olean`/`.ilean` reader re-evaluation

---

## Cross-cutting (check off when fully done across all phases)
- [x] Every feature has a JSON output mode
- [ ] Every path from API/MCP is checked against workspace roots
- [x] No arbitrary shell execution; commands built from typed options only
- [ ] Every Lean process has timeout, memory limit (where supported), cancellation, output limit, sanitized env
- [ ] Index and cache treated as acceleration only; Lean is the source of truth
- [x] No Apache-only (or other disallowed) dependency in the tree
- [ ] Unit, golden, integration, FFI, and MCP/API schema tests in place (spec section 17)
- [ ] Windows, Linux, macOS all green in CI
