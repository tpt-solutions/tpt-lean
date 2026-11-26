# Changelog

All notable changes to `tpt-lean-server` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

Initial development version `0.1.0`. Not yet published; see [PUBLISHING.md](../../PUBLISHING.md).

### Added
- JSON-RPC 2.0 core over newline-delimited messages.
- Transports: Unix domain socket by default on Linux and macOS (owner-only, 0600); Windows named
  pipe by default on Windows; loopback TCP with `--port`; loopback HTTP with `--http PORT`
  (`POST /v1/rpc`, with a Host check).
- Lifecycle: `server start` records its state, so `status` and `stop` work with any transport.
  State lives in `.tpt-lean/server.json`.
- Remote access disabled by default (`server.allow_remote`).
- Permission classes checked per method.
- Path safety through `WorkspaceRoot`; only `.lean` files accepted.
- Request size limit.
- JSON Schemas for request and response shapes, and the error envelope.
- Methods for project info and doctor, search, index status and refresh, formatting, Lean checks,
  goals, diagnostics, imports (including `imports.unused`), context packs, `build.subscribe`, and
  shutdown (see `docs/server-api.md`).
- Warm Lean session per project for `context.pack` and `diagnostics.explain`, stopped after 10
  minutes idle. Measured: repeated text call p50 1 ms; after an edit p50 231 ms, p95 234 ms; the
  first call 4.7 to 6.0 s.
- Client module for calling a running server.

### Known limitations
- No server-sent event stream. Build events use JSON-RPC notifications.
- Some methods are still being wired; see `todo.md`.
