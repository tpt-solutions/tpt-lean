# JSON output

Every command accepts `--json`. The output is one JSON document on stdout, in the envelope below.
Some commands also have a streaming form, and a few deliberately use their own format; those are
listed at the end.

The HTTP and JSON-RPC API uses its own shapes; see [server-api.md](server-api.md) and the schemas in
`crates/tpt-lean-server/schemas/`.

## Envelope

A success:

```json
{"schema_version":1,"tool":"tpt-lean","kind":"search","ok":true,"data":{"count":1,"items":[{"docstring":null,"file":"Main.lean","kind":"def","line":1,"name":"main","package":"project","score":0.64}]}}
```

A failure, with `--json`:

```json
{"schema_version":1,"tool":"tpt-lean","kind":"error","ok":false,"error":{"code":"invalid_argument","message":"usage: tpt-lean search <query> | --type PATTERN | ...","path":null}}
```

- `schema_version` is `1`. It changes only when an existing field is renamed, removed, or changes
  meaning. Adding a field does not change it.
- `tool` is always `tpt-lean`.
- `kind` names the result. Clients should dispatch on it.
- `ok` is `false` only for an error envelope. A command can exit non-zero with `ok: true`: for
  example `fmt --check` reports `"changed": true` and exits 1 when a file would change.
- `data` holds the result. List results have `count` and `items`.
- `path` is the file or directory the error concerns, or `null`.

Clients should ignore fields they do not know. Field names are stable.

## Error codes

These appear as `error.code`. The exit code is in [cli.md](cli.md).

| Code | Meaning |
| --- | --- |
| `invalid_argument` | bad argument, unknown command or flag (exit 2) |
| `io` | a file could not be read or written (exit 9) |
| `config_invalid` | configuration is invalid (exit 4) |
| `project_not_found` | no project at the given root (exit 3) |
| `path_outside_workspace` | a path resolves outside the project (exit 5) |
| `lean_not_found` | `lean` or `lake` was not found, or a Lean process failed to start (exit 3) |
| `index_error` | the index could not be read or written (exit 6) |
| `syntax_invalid` | the source cannot be read safely (exit 7) |
| `timeout` | a process ran past its timeout (exit 8) |
| `output_limit_exceeded` | a process wrote more output than allowed (exit 8) |
| `not_implemented` | reserved; no command returns it now |
| `internal` | an unexpected failure (exit 70) |

## Kinds

Each kind below lists the fields of `data`. Samples come from the current build.

| `kind` | `data` |
| --- | --- |
| `search` | `count`, `items`: each item has `name`, `kind`, `package`, `file`, `line`, `score` (0 to 1), `docstring` (or `null`) |
| `search` with `--type` | `count`, `items`: `name`, `kind`, `package`, `file`, `type_text`, `score` |
| `imports` | `count`, `items`: `module`, `file`, `line` |
| `imports.cost` | `count`, `items`: `module`, `file`, `direct_imports`, `external_imports`, `transitive_project_imports`, `fan_in`, `heavy` |
| `imports.unused`, `imports.suggest`, `imports.minimize` | see the `imports` section of [cli.md](cli.md) |
| `fmt` | `count`, `items`: `file`, `changed` (true when the formatter would change it), `error` (or `null`) |
| `index.refresh` | `scanned`, `updated`, `unchanged`, `removed`, `db` |
| `index.status` | `files`, `symbols`, `imports`, `packages`, `integrity_ok`, `schema_version` |
| `index.sidecar` | see the `index` section of [cli.md](cli.md) |
| `doctor` | `count`, `items`: `name`, `status` (`ok`, `warn` or `fail`), `detail` |
| `doctor --fix` | the same items; each applied fix is an extra item named `fix` with its `detail` |
| `cache.status` | `location`, `exists`, `files`, `bytes`, `shared` (the shared layer directories) |
| `cache.prune`, `cache.export`, `cache.import` | see the `cache` section of [cli.md](cli.md) |
| `goals` | `file`, `line`, `column`, `goals` (an array of goal strings; empty when none), `target` (`tactic`, `term`, `statement` or `none`; present with `--sidecar` only) |
| `fix` | `plan` (`file`, `steps` (each `kind`, `line` or `null`, `detail`), `diff`), `errors_before`, `unresolved`, `applied`, `rolled_back`, `errors_after` (or `null`) |
| `explain` | `lean` (`version`, `compatibility`), `error` (or `null`), `context` (`goal`, `hypotheses`), `suggestions` |
| `explain.last_error` | the diagnostics saved by the last `check`; see the `explain` section of [cli.md](cli.md) |
| `context-pack` | the context pack: `file`, `position`, `snippet`, `goal`, `hypotheses`, `diagnostics`, `imports`, `nearby_definitions`, `relevant_lemmas`, `constraints`, `truncated`, `approx_tokens`, `lean_compatibility` |
| `build.plan` | `modules`, `waves` (module names per wave), `critical_path`, `widest_wave` |
| `build.explain_slow` | see the `build` section of [cli.md](cli.md) |
| `build.report` | `success`, `cache_hit`, `cache_layer` (`memory`, `disk`, `shared` or `null`), `key_ms`, `total_ms`, `lake_ms`, `files_built`, `files_failed`, `slowest` (each `file`, `duration_ms` or `null`, `failed`) |
| `watch` | one record per checked file, streamed as each check finishes: `file`, `errors`, `warnings`, `sorry`, `error` (or `null`), `diagnostics` |
| `sorry-report` | `count`, `items`: `file`, `line`, `column`, `keyword` (`sorry` or `admit`), `declaration` (or `null`); with `--sidecar`, also `goals` (`line`, `column`, `target`, `goals`) or `goals_error` |
| `setup` | `count`, `items`: `path`, `action` (`create`, `update` or `unchanged`), `notes` |
| `completions` | `shell`, `script` (the text of the completion script) |
| `profile.compare`, `profile.flamegraph`, `profile.graph` | see the `profile` section of [cli.md](cli.md) |
| `bisect` (`--axis commits`) | `axis`, `good`, `bad`, `commits_between`, `probes` (each `{commit, bad}`), `first_bad`, `scope`, `not_done` |
| `bisect` (`--axis imports` or `flags`) | `axis`, `file`, `candidates`, `probes` (each `{enabled, item, bad}`), `first_bad` (the candidate name), `scope`, `not_done` |
| `docgen` | `project`, `modules`, `declarations`, `namespaces`, `import_edges`, `external_imports`, `files_written`, `out_dir` |
| `config` | the effective configuration |
| `clean` | `ran`, `exit_code` |
| `server.status` | `running`, `stale`, `pid`, `port`, `socket`, `pipe`, `http_port`, `project` |
| `server.stop` | `outcome` (`not_running`, `stopped`, `stale_removed` or `timed_out`) and `pid` |
| `help` | `commands` (the number of lines in the help text) |
| `init` | `root` and `files` (the files written) |
| `error` | see the envelope above |

Kinds that are not in the table use the same envelope; their `data` is described by the command's
section in [cli.md](cli.md).

## Build events

`build --json` prints one event per line instead of an envelope. Each event has `event`, `run_id`
and `timestamp_ms`, and the fields below.

| `event` | Other fields |
| --- | --- |
| `build_started` | `target` (when given) |
| `cache_hit` | `digest`, `layer` (`memory`, `disk` or `shared`) |
| `cache_miss` | `digest` |
| `file_started` | `file` |
| `file_finished` | `file`, `duration_ms` (when known) |
| `file_failed` | `file`, `duration_ms` (when known) |
| `diagnostic` | `severity`, `file`, `line`, `column`, `message` |
| `build_finished` | `success`, `duration_ms`, `files_built`, `files_failed` |

Sample:

```json
{"event":"file_started","run_id":"build_1795645832805_0","file":"Mods/Base.lean","timestamp_ms":1795645835412}
{"event":"build_finished","run_id":"build_1795645832805_0","timestamp_ms":1795645854013,"success":true,"duration_ms":21208,"files_built":41,"files_failed":0}
```

The same events are streamed by the API server (`GET /v1/events` and `build.subscribe`); see
[server-api.md](server-api.md).

## Commands with their own format

- `ci --json` prints the CI report, without the envelope. Its fields are `schema_version` (`1`),
  `ok`, `project`, `toolchain` (`lean` and `lake` paths), `build` (`status`, `exit_code`,
  `duration_ms`), `total_ms`, `diagnostics`, `failures`, and `cache` (`available`, `hits`, `misses`,
  `entries`, `note`). Each diagnostic has `file`, `line`, `column`, `severity` and `message`.
- `ci --sarif` prints a SARIF 2.1.0 log. `scripts/validate-sarif.py` checks it against the official
  schema.
- `build --json` prints build events, as above, and no envelope. The summary is the final
  `build_finished` event.
- `profile FILE --json` prints the profile report itself, with `format_version` (currently `1`), not
  the envelope. `--flamegraph` writes folded stacks to a file.
- `imports --ndjson` prints one record per line: `{"kind":"imports","item":{...}}`. Other list commands
  print the same way under `--ndjson`.
