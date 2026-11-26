# tpt-lean-build

Build orchestration for Lake projects, part of [tpt-lean](https://github.com/tpt-solutions/tpt-lean).

This crate runs `lake build` through `tpt-lean-adapter` with typed arguments (no shell), turns
Lake's progress output into a structured event stream, computes cache keys for a project, and
records successful builds in a local cache. A watch mode uses the same machinery to rebuild when
sources change. A plan command shows the module import order, and a report shows where the time
went.

Lake decides what needs rebuilding. The cache never serves build outputs; it reports hits and
misses, and watch mode uses it to skip work whose inputs are unchanged.

## Status

Early development (`0.1.0`, not yet published). Building, the event stream, cache keys, the disk
and shared caches, project scanning, watch mode, the build plan, and the build report are
implemented and tested against a real Lake project.

## Event stream

Each build emits JSON events, one per line, in this order:

1. `build_started`
2. `cache_hit` or `cache_miss`
3. per module: `file_started`, then `file_finished` or `file_failed`
4. `diagnostic` for each Lean message
5. `build_finished`

Events are emitted while Lake runs: the adapter passes Lake's output lines on as they are read
(`tpt-lean-adapter::run_streaming`).

Every event has `event`, `run_id`, and `timestamp_ms`. File events include `file` (project
relative when possible, otherwise the module name) and `duration_ms` when Lake reports one.

Example:

```json
{"event":"file_finished","run_id":"build_1795487124584_0","file":"Demo/Basic.lean","timestamp_ms":1795487133842,"duration_ms":1200}
```

Lake prints no start line for successful modules, so `file_started` is derived from the reported
duration and its timestamp is approximate.

## Cache

- Cache keys: `cache_key_for` hashes the project's inputs with SHA-256 (`CacheKey`). Tool
  versions are memoized per binary, so a warm key computation does not start Lean or Lake. The
  JSON output carries `key_ms`, the time the key took.
- Memory layer (`MemoryCache`): recent successful digests in the process; used by watch mode.
- Disk layer (`DiskCache`): one JSON record per digest under `<global cache>/build/records/`.
  A record is accepted only for the same project root.
- Shared layer: `TPT_LEAN_SHARED_CACHE` names one or more directories of build records, for a team
  share. Lookups go memory, then local disk, then shared. A shared hit is matched on its digest and
  copied to the local disk layer.
- `CacheLayer` identifies which layer answered a lookup.
- `status` reports size and entry counts (`CacheStatus`); `prune_older_than` removes old records
  and returns a `PruneReport`.

A `cache_hit` means the key matched an earlier successful build. Lake still runs, so the build's
time is Lake's own.

## Build plan and report

- `build --plan [--json]` prints the project's module import waves, the critical path, and the
  widest wave (`schedule` module).
- `build --report` prints the cache layer that served the build, the cache-key time, Lake's share
  of the total time, and the ten slowest files.

## Project scanning

`scan_project` returns a `ProjectScan` listing `ProjectFile` entries, `manifest_packages` reads
the Lake manifest, and `module_name` maps a path to its Lean module name.

## Watch mode

`watch` polls at a configurable interval (minimum 50 ms) and compares content hashes of the
sources, the lakefile, the lock file, and the toolchain pin. When something changed, it
recomputes the key and skips the rebuild if that key already built successfully in this session.
It polls with the standard library; it is not an OS file watcher.

## Other helpers

- `probe_tools` checks that `lean` and `lake` run and reports their versions (`Tools`).
- `parse_output`, `parse_line`, and `parse_duration_ms` read Lake's output.
- `explain_slow` names the slowest files in a build (`SlowFile`).

## Usage

```
tpt-lean build [target] [--json] [--watch] [--poll-ms N] [--plan] [--report] [--explain-slow] [--timeout S]
tpt-lean cache status
tpt-lean cache prune [--days N] [--dry-run]
tpt-lean cache export DIR
tpt-lean cache import DIR
```

```toml
[dependencies]
tpt-lean-build = "0.1"
```

## Limits

- Parallel orchestration beyond Lake's own job pool is not implemented.
- Deep build graphs are not analyzed beyond the import waves of `--plan`.
- Measured, not yet explained: a full `tpt-lean build` of a 41-module project takes 17 to 20 s,
  against about 7 s for `lake build` from the same shell. The slowdown follows the parent
  process of Lake: a shell or a direct launch is fast, and a Rust, Python or cmd parent is slow.
  The cause is not known.
- Measured, not yet explained: a cache-hit build takes about 2.6 s on a small project, against
  about 1.1 s for a `lake build` that does nothing. The cache key takes about 1 ms; the extra
  time is in the Lake process that tpt-lean starts.

## Dependencies

| Crate | Purpose |
| --- | --- |
| `tpt-lean-core` | configuration, cache directory, hashing |
| `tpt-lean-adapter` | running `lake build` |
| `tpt-lean-syntax` | module names from source |
| `serde`, `serde_json` | event and record format |

## Testing

```
cargo test -p tpt-lean-build
```

The crate has unit tests. `tests/real_lake.rs` creates a real Lake project and builds, rebuilds,
and watches an edit; it needs Lake installed and redirects the cache to a temporary directory.

## Related documentation

- [Build orchestration guide](https://github.com/tpt-solutions/tpt-lean/blob/master/docs/build.md)

## License

Licensed under either of [MIT](https://opensource.org/licenses/MIT) or
[Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0), at your option.
