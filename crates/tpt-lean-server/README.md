# tpt-lean-server

A local JSON-RPC 2.0 API server for [tpt-lean](https://github.com/tpt-solutions/tpt-lean). It
exposes the same operations as the CLI (project info, search, formatting, index status, goals,
diagnostics, context packs, builds) to local programs without a new process per request.

The server is for tools that want to keep talking to one project. Lean remains the authority for
every result.

## Status

Early development (`0.1.0`, not yet published). The JSON-RPC core, the transports, permission
checks, path safety, lifecycle commands, and request and response schemas are implemented and
tested. Not every method in the API is fully wired yet; `docs/server-api.md` and `todo.md` track
which are.

## Transports

Messages are newline-delimited JSON-RPC 2.0. The server offers these transports:

- **Unix domain socket**: the default on Linux and macOS. The socket file is owner-only (0600).
- **Windows named pipe**: the default on Windows (`\\.\pipe\tpt-lean-<hash>`, see
  `tpt-lean-pipe`).
- **Loopback TCP**: `tpt-lean server start --port N`, bound to `127.0.0.1`.
- **Loopback HTTP**: `tpt-lean server start --http PORT`, serving `POST /v1/rpc`. Requests whose
  Host header is not loopback are refused.

Remote access is off by default. Setting `server.allow_remote` is required to listen on any
other address. Requests larger than `MAX_REQUEST_BYTES` are rejected.

## Methods

The method names and parameter shapes are in [`docs/server-api.md`](https://github.com/tpt-solutions/tpt-lean/blob/master/docs/server-api.md),
with JSON Schemas for each in `schemas/`. Current methods:

| Group | Methods |
| --- | --- |
| Project | `project.info`, `project.doctor` |
| Search | `search.query`, `search.symbol`, `search.goal` |
| Index | `index.status`, `index.refresh` |
| Formatting | `format.text`, `format.file` |
| Lean | `check.file`, `check.project`, `goals.at` |
| Diagnostics | `diagnostics.get`, `diagnostics.explain` |
| Imports | `imports.suggest`, `imports.unused` |
| Context | `context.pack` |
| Build | `build.subscribe` (events as `build.event` notifications, then `build.finished`) |
| Lifecycle | `server.shutdown` |

## Warm Lean session

`context.pack` and `diagnostics.explain` share a warm Lean session per project. The session is
reused while requests keep arriving, re-elaborated after edits, and stopped after 10 minutes idle
or at shutdown. Measured through the server: a repeated text call takes p50 1 ms; after an edit,
p50 231 ms and p95 234 ms; the first call, which starts Lean, takes 4.7 to 6.0 s.

## Permissions

Each method belongs to a permission class (`Permission`), and a request whose class the
configuration does not grant returns `PERMISSION_DENIED`. The control method `server.shutdown` is
always allowed on loopback. Read methods are allowed by default; write and build methods need
`[security]` opt-ins, as in `tpt-lean-mcp`.

## Path safety

Every path parameter is resolved through `WorkspaceRoot`. Paths outside the project root return
`PATH_OUTSIDE_WORKSPACE`. Only `.lean` files are accepted.

## Lifecycle

```
tpt-lean server start [--port N] [--http PORT]
tpt-lean server status
tpt-lean server stop
```

With no flags, `start` uses the default transport for the platform (socket on Linux and macOS,
named pipe on Windows). After binding, the server writes `.tpt-lean/server.json` with the
transport, `pid`, `project`, and `version`. `status` and `stop` read that file, whichever
transport was used. `stop` sends `server.shutdown` and waits up to ten seconds for the state file
to clear.

## Errors

Errors use standard JSON-RPC codes plus application codes such as `PERMISSION_DENIED`,
`PATH_OUTSIDE_WORKSPACE`, and `APP_ERROR` (for a missing index, a failed Lean start, or I/O).
`schemas/error.json` describes the envelope.

## Client

The `client` module provides a small blocking client that `tpt-lean` and tests use to call a
running server. It connects over the transport the server recorded, sends `call` requests, and
checks responses with `is_ok_response`.

## Usage

```toml
[dependencies]
tpt-lean-server = "0.1"
```

Example request (one JSON object per line):

```json
{"jsonrpc":"2.0","id":1,"method":"project.info","params":{}}
```

## Limits

- No server-sent event stream. Build events go over JSON-RPC notifications (`build.subscribe`).
- Some methods are still being wired; see `todo.md`.

## Dependencies

Workspace crates `tpt-lean-core`, `tpt-lean-index`, `tpt-lean-search`, `tpt-lean-fmt`,
`tpt-lean-syntax`, `tpt-lean-adapter`, `tpt-lean-profile`, `tpt-lean-build`, and `tpt-lean-tools`,
plus `serde_json`. On Windows, also `tpt-lean-pipe`.

## Testing

```
cargo test -p tpt-lean-server
```

- `tests/api.rs` runs requests against a live listener.
- `tests/schemas.rs` checks request and response shapes against `schemas/`.
- `tests/bench.rs` holds the warm-session timings (run with `--ignored` in release mode).

## License

Licensed under either of [MIT](https://opensource.org/licenses/MIT) or
[Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0), at your option.
