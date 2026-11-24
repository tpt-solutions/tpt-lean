# Build orchestration and cache (`tpt-lean-build`)

`tpt-lean-build` runs `lake build` through the adapter (typed arguments, no shell), turns Lake's
progress output into a structured event stream, computes a cache key for the project, and records
successful builds in a local disk cache.

Lake stays the authority on what needs rebuilding. The cache never serves build outputs; it reports
hits and misses, and watch mode uses it to skip rebuilds whose inputs are unchanged.

## Event stream

Every build emits events in this order: `build_started`, `cache_hit` or `cache_miss`, then per
module `file_started`, `file_finished` or `file_failed`, then `diagnostic` for each Lean message,
and finally `build_finished`. Each event is one JSON object with `event`, `run_id` and
`timestamp_ms`:

```json
{"event":"file_finished","run_id":"build_1795487124584_0","file":"Demo/Basic.lean","timestamp_ms":1795487133842,"duration_ms":1200}
```

`file` is the source path relative to the project root when the module is part of the project,
otherwise the module name. `duration_ms` is present when Lake reports one.

Known limits:

- The adapter captures Lake's output until Lake exits, so file events are replayed after the build.
  Live progress needs a streaming option in the adapter.
- Lake prints no start line for successful modules. `file_started` is derived from the reported
  duration, so its `timestamp_ms` is approximate.
- Non-module jobs (`Demo.Basic:c.o`, `demo:exe`) are counted but not emitted as file events.

## Lake progress format

Observed with Lake 5.0.0 / Lean 4.36.0 on Windows (stdout):

```text
✔ [2/11] Built Demo.Basic (542ms)
✖ [2/11] Building Demo.Basic (505ms)
Build completed successfully (11 jobs).
```

`✖ ... Building` marks a failed module; diagnostics follow it. Only `ms` durations were observed.
Unknown lines are ignored.

## Cache key

`cache_key_for` hashes these inputs with SHA-256 (`tpt_lean_core::hash::CacheKey`):

| Field | Source |
| --- | --- |
| `lean_version` | `lean --version` |
| `lean_flags` | Lake version and target |
| `target_triple` | architecture, OS, and platform reported by `lean --version` |
| `package_versions` | `name@rev` entries of `lake-manifest.json` |
| `lakefile_hash` | `lakefile.lean` / `lakefile.toml` |
| `dependency_lock_hash` | `lake-manifest.json` |
| `source_hash` | every project `.lean` file (path and content) |
| `import_hashes` | content hash of each imported project module; external imports by name |
| `env` | variables prefixed `LEAN_`, `LAKE_`, `ELAN_` |

Tool versions are memoized per binary (path, size, modification time) under the global cache
directory, so a warm key computation does not start Lean or Lake.

## Cache layers

- Memory: process-local map of recent successful digests, used by watch mode.
- Disk: one JSON record per digest under `<global cache>/build/records/`, holding the project root,
  time, duration and file count. A record is accepted only for the same project root.
- Not implemented: shared team cache and CI cache.

On a successful build both the digest before and after Lake ran are recorded, because Lake can
create a key input (for example `lake-manifest.json` on the first build).

`cache_hit` means the key matched a recorded success. Lake still runs, so total time is Lake's own
no-op time; check `files_built` for what actually rebuilt.

## Watch mode

`watch` polls every `poll_ms` (minimum 50 ms) and compares content hashes of the sources, lakefile,
lock file and toolchain pin. Only `std` is used; no file watcher crate. When something changed, the
key is recomputed and the rebuild is skipped if that key already built successfully in this
process. Hashing every file on each poll costs time proportional to project size.

## Public API

- `build(project_root, target, timeout_secs, sink) -> BuildOutcome`
- `cache_key_for(project_root, target) -> CacheKey`
- `watch(project_root, target, timeout_secs, poll_ms, cancel, sink)`
- `explain_slow(events, n) -> Vec<SlowFile>`: modules ranked by reported duration.
- `status(dir) -> CacheStatus`, `prune_older_than(dir, age, dry_run) -> PruneReport`, with
  `records_dir()` for the build records location.
- `BuildEvent` (serde, tagged by `event`), `BuildOutcome`, `SlowFile`.

## Tests

- Unit tests: parsing of captured Lake output, event JSON shape, key stability and invalidation,
  disk store/lookup/prune in temporary directories, watch fingerprinting.
- Real toolchain (ignored by default): `cargo test -p tpt-lean-build --test real_lake -- --ignored --nocapture`
  runs `lake new`, builds, rebuilds unchanged, edits a module, and watches an edit. It redirects the
  cache into a temporary directory.
