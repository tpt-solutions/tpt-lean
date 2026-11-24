# tpt-lean-server

A local JSON-RPC 2.0 API server for [tpt-lean](https://github.com/tpt-solutions/tpt-lean). It
exposes the same operations as the CLI (project info, search, formatting, index status, goals,
diagnostics, context packs) to local programs over newline-delimited JSON on TCP.

The server is for tools that want to keep talking to one project without launching a new
process per request. Lean remains the authority for every result.

## Status

Early development (`0.1.0`, not yet published). The JSON-RPC core, permission checks, path
safety, lifecycle commands, and request and response schemas are implemented and tested. Not
every method in the API is fully wired yet; `docs/server-api.md` and `todo.md` track which are.

## Transport

- Newline-delimited JSON-RPC 2.0 over TCP bound to `127.0.0.1`.
- Remote access is off by default. Setting `server.allow_remote` is required to listen on any
  other address.
- Requests larger than `MAX_REQUEST_BYTES` are rejected.

Not implemented: Unix domain sockets, Windows named pipes, server-sent event streams, and a
persistent daemon with a warm index.

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
| Imports | `imports.suggest` (`imports.unused` is not implemented) |
| Context | `context.pack` |
| Lifecycle | `server.shutdown` |

## Permissions

Each method belongs to a permission class (`Permission`), and a request whose class the
configuration does not grant returns `PERMISSION_DENIED`. The control method `server.shutdown` is
always allowed on loopback.
Read methods are allowed by default; write and build methods need `[security]` opt-ins, as in
`tpt-lean-mcp`.

## Path safety

Every path parameter is resolved through `WorkspaceRoot`. Paths outside the project root return
`PATH_OUTSIDE_WORKSPACE`. Only `.lean` files are accepted.

## Lifecycle

```
tpt-lean server start [--port N]     # 7373 by default, on 127.0.0.1
tpt-lean server status
tpt-lean server stop
```

After binding, the server writes `.tpt-lean/server.json` with `pid`, `port`, `project`, and
`version`. `status` and `stop` read that file. `stop` sends `server.shutdown` and waits up to ten
seconds for the state file to clear.

## Errors

Errors use standard JSON-RPC codes plus application codes such as `PERMISSION_DENIED`,
`PATH_OUTSIDE_WORKSPACE`, and `APP_ERROR` (for a missing index, a failed Lean start, or I/O).
`schemas/error.json` describes the envelope.

## Client

The `client` module provides a small blocking client that `tpt-lean` and tests use to call a
running server. It sends `call` requests and checks responses with `is_ok_response`.

## Usage

```toml
[dependencies]
tpt-lean-server = "0.1"
```

Example request (one JSON object per line):

```json
{"jsonrpc":"2.0","id":1,"method":"project.info","params":{}}
```

## Dependencies

Workspace crates `tpt-lean-core`, `tpt-lean-index`, `tpt-lean-search`, `tpt-lean-fmt`,
`tpt-lean-syntax`, and `tpt-lean-adapter`, plus `serde_json`.

## Testing

```
cargo test -p tpt-lean-server
```

- `tests/api.rs` runs requests against a live listener.
- `tests/schemas.rs` checks request and response shapes against `schemas/`.

## License

Licensed under either of [MIT](https://opensource.org/licenses/MIT) or
[Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0), at your option.
