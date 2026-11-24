# Changelog

All notable changes to `tpt-lean-server` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

Initial development version `0.1.0`. Nothing has been published yet.

### Added
- JSON-RPC 2.0 core over newline-delimited TCP on `127.0.0.1`.
- Remote access disabled by default (`server.allow_remote`).
- Permission classes checked per method.
- Path safety through `WorkspaceRoot`; only `.lean` files accepted.
- Request size limit.
- Lifecycle: foreground run with `.tpt-lean/server.json`, status, and stop.
- JSON Schemas for request and response shapes, and error envelope.
- Methods for project info and doctor, search, index status and refresh, formatting, Lean
  checks, goals, diagnostics, imports, context packs, and shutdown (see `docs/server-api.md`).
- Client module for calling a running server.

### Known limitations
- `imports.unused` is not implemented.
- No Unix domain socket or named-pipe transport, no event streams, no persistent daemon.
- Some methods are still being wired; see `todo.md`.
