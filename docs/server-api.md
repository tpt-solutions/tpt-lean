# tpt-lean API server (`tpt-lean-server`)

JSON-RPC 2.0 over TCP. One request per line, one response per line (newline-delimited JSON).
The server listens on `127.0.0.1` unless `server.allow_remote = true`.

Schemas for every method's params and result are in `crates/tpt-lean-server/schemas/`
(`error.json` for the error envelope). Tests in `tests/schemas.rs` validate real responses.

## Lifecycle

The server writes `.tpt-lean/server.json` (`pid`, `port`, `project`, `version`) after it binds,
and removes it on exit. Port 0 picks a free port; the file records the real one.

Public functions for the CLI:

- `run_foreground(root, port) -> Result<()>`: serve until `server.shutdown`. Refuses to start if
  a live server already answers for the project.
- `status(root) -> Result<StatusReport>`: probes with `project.info`. `StatusReport::to_json()`
  gives `running`, `stale`, `pid`, `port`, `project`.
- `stop(root) -> Result<StopReport>`: sends `server.shutdown` and waits up to 10 s for the state
  file to disappear. `StopReport::to_json()` gives `outcome` (`not_running`, `stopped`,
  `stale_removed`, `timed_out`) and `pid`.

## Permissions

Each method has a class. The check happens before dispatch.

| Class | Methods | Granted when |
| --- | --- | --- |
| read | `project.info`, `index.status`, `search.query`, `search.symbol`, `format.text`, `diagnostics.get` | always |
| write | `format.file`, `index.refresh` | `security.allow_write = true` (default false) |
| build | `check.file`, `goals.at` | `security.allow_build = true` (default true) |
| control | `server.shutdown` | always (loopback unless remote is allowed) |

A denied request gets error `-32001` with `data: {"permission", "config_key"}`.

## Path parameters

Every path parameter is resolved through `WorkspaceRoot`: relative paths are joined to the
project root, `..` is normalized, and symlinks are resolved before the containment check. A path
outside the root gets `-32002`. Paths must end in `.lean` (otherwise `-32602`), and NUL bytes
are rejected.

## Methods

| Method | Params | Result summary |
| --- | --- | --- |
| `project.info` | none | `root`, `marker`, `server_version`, `methods`, `permissions` |
| `index.status` | none | `schema_version`, `files`, `symbols`, `packages`, `integrity_ok` |
| `index.refresh` | none | `scanned`, `updated`, `unchanged`, `removed` |
| `search.query` | `query`, `limit?` (1-200) | `items[]` with `name`, `kind`, `source`, `line`, `score` |
| `search.symbol` | `name`, `limit?` | `items[]`: exact or prefix name, `short_name`, `namespace`, `package`, `source`, `line` |
| `format.text` | `text` | `formatted`, `changed` |
| `format.file` | `path` | `path`, `changed`, `written`. Refuses uncertain syntax; writes via temp file and rename |
| `check.file` | `path` | `ok`, `exit_code`, `timed_out`, `output_truncated`, `diagnostics[]` (runs `lean <file>`) |
| `diagnostics.get` | `path` | `known`, `diagnostics[]` from the last `check.file` in this server process |
| `goals.at` | `path`, `line`, `column` (both 1-based) | `goals[]` (plain goals from the Lean language server) |
| `server.shutdown` | none | `stopping: true` |

`check.file` and `goals.at` need a Lean toolchain. Without one they return `-32000` with a
message naming `lean`.

## Error codes

| Code | Meaning |
| --- | --- |
| -32700 | Parse error |
| -32600 | Invalid request (bad `jsonrpc`, missing `method`, request over 1 MiB) |
| -32601 | Method not found |
| -32602 | Invalid params |
| -32000 | Application error (no index, Lean missing, uncertain format input, I/O) |
| -32001 | Permission denied |
| -32002 | Path outside the project root |

## Example

```json
{"jsonrpc":"2.0","id":"1","method":"search.query","params":{"query":"add_comm","limit":5}}
{"jsonrpc":"2.0","id":"1","result":{"items":[{"name":"Nat.add_comm","kind":"theorem","source":"Mathlib/...","line":12,"score":0.94}]}}
```

## Not implemented

- Methods: `project.doctor`, `search.goal`, `check.project`, `diagnostics.explain`,
  `imports.suggest`, `imports.unused` (out of scope for this phase), `context.pack`,
  `build.*`, `profile.*`.
- Transports: Unix domain sockets and named pipes. Only TCP is implemented.
- Streaming (`GET /v1/events`, SSE or notifications).
- HTTP (`POST /v1/rpc`).
- Authentication. Any local process that can connect to the port can call the methods allowed
  by the configuration. Keep write and build permissions off unless you need them.
