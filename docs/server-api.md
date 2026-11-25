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
| read | `project.info`, `index.status`, `search.query`, `search.symbol`, `format.text`, `diagnostics.get`, `profile.get`, `build.status`, `imports.suggest`, `imports.unused` | always |
| write | `format.file`, `index.refresh` | `security.allow_write = true` (default false) |
| build | `check.file`, `goals.at` | `security.allow_build = true` (default true) |
| profile | `profile.run` | `security.allow_build` and `security.allow_profile` (both default true) |
| build | `build.run`, `build.cancel`, `check.project`, `diagnostics.explain`, `context.pack`, `project.doctor`, `search.goal` | `security.allow_build = true` (default true) |
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
| `profile.run` | `path` | the profile report, as `tpt-lean profile --json`, plus `id`; the report is also stored in `.tpt-lean/profiles/` (runs Lean on an instrumented copy of the file; bounded by `security.command_timeout_seconds`) |
| `build.run` | optional `target` | `run_id` and `status: "running"` at once; `lake build` runs on a background thread. One build at a time; a second `build.run` is refused while one runs |
| `build.status` | optional `run_id` (default: latest) | `status` (`none`, `running`, `passed`, `failed`, `cancelled`) and `result` (success, exit code, digest, cache hit, files built and failed, duration, error) once finished |
| `build.cancel` | optional `run_id` (default: newest running build) | `cancelled` true when a running build was asked to stop; the build then finishes as `cancelled` |
| `check.project` | optional `timeout` (1 to 3600 s; default `security.command_timeout_seconds`) | the CI report for `lake build` of the whole project: `ok`, `build` status and exit code, `diagnostics`, `failures`, and cache counters. It blocks until the build finishes or times out; `build.run` is the background variant |
| `diagnostics.explain` | `path`, `line`, `column` (both 1-based) | `error` at that position (or null), `context` with the goal and hypotheses, lemma `suggestions` from the index, and `lean` (`version`, and `compatibility`: `tested`, `newer`, `older`, `between` or `unknown`) (runs the Lean language server) |
| `context.pack` | `path`, `line`, `column`, optional `max_tokens` (default 6000) | a token-budgeted bundle: snippet (never dropped), goal, hypotheses, diagnostics, imports, nearby declarations, lemma candidates, and `truncated` / `approx_tokens` (runs the Lean language server) |\n| `project.doctor` | none | `ok` (false when a check failed) and `checks[]` (`name`, `status`, `detail`): the same checks as `tpt-lean doctor`. Some checks start the language server, and the write check creates `.tpt-lean` when it is missing |\n| `search.goal` | `path`, `line`, `column`, optional `limit` (1 to 50, default 5) | `goal` and `hypotheses` at the position, `results[]` of lemma candidates for the goal's target from the index, and `indexed` (false when the project has no index yet) |\n| `imports.suggest` | `query`, optional `limit` (1 to 50, default 10) | `suggestions[]`: modules from the index that declare matching names (run `index.refresh` first) |
| `imports.unused` | `path` | `findings[]` with code `import_unused` for imports the file does not use; imports from outside the project are reported as `import_unresolved` |
| `profile.get` | optional `id` | without `id`: `ids` of stored profiles, newest first, and `report: null`; with `id`: the stored `report`. Ids are restricted to letters, digits, `-` and `_` |
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
