# tpt-lean-index

A persistent, local symbol index for Lean 4 projects and their local dependencies, stored in
SQLite. Part of [tpt-lean](https://github.com/tpt-solutions/tpt-lean).

The index records syntax-level facts produced by `tpt-lean-syntax`: files, declarations,
imports, docstrings, and packages. It makes name and docstring search fast across a large
project and its `.lake/packages` dependencies, including Mathlib once indexed locally.

The index is an accelerator. It never claims mathematical truth. Anything it returns should be
confirmed by Lean.

## Status

Early development (`0.1.0`, not yet published). Scanning, incremental refresh, the FTS5
search table, status reporting, the health check used by `tpt-lean doctor`, and the Lean sidecar
storage are implemented and tested. The limits are listed below.

## What it stores

The schema (version `SCHEMA_VERSION`) has these tables:

| Table | Contents |
| --- | --- |
| `packages` | the project and each local dependency |
| `files` | scanned `.lean` files with content hashes |
| `symbols` | declarations with kind, namespace, range, and docstring |
| `symbol_text` | FTS5 virtual table over names and docstrings |
| `imports` | import edges between modules |
| `diagnostics` | stored diagnostics, when recorded |
| `build_runs`, `profile_runs` | history rows written by the build and profile features |
| `cache_entries` | cache bookkeeping |

The database runs in **WAL mode**, so readers keep working while a writer refreshes.

## What it provides

- **Scanning** (`collect_lean_files`, `default_scan_roots`, `ScanRoot`): finds `.lean` files
  under the project and under `.lake/packages`. Directories named `.lake`, `build`, and
  `target` are skipped, and `.gitignore` files in the project and its subdirectories are
  honoured (`ignore` module).
- **Index handle** (`Index`): open, refresh, status, clear. `refresh` is incremental: files
  whose content hash has not changed are not reparsed. `RefreshStats` reports what changed.
- **Status** (`Status`): counts and schema information, used by `tpt-lean index status` and
  the server's `index.status` method.
- **Symbol rows** (`SymbolRow`): the record type that `tpt-lean-search` consumes.
- **Declarations** (`store_declarations`, `declarations_named`, `modules_needing_sidecar`):
  Lean sidecar output per module, with the source hash, import-closure hash, toolchain and
  sidecar schema it was produced from. Stale data is dropped on every refresh.
- **Full-text queries** (`fts_query`): converts user text into a safe FTS5 query, so
  punctuation in a search string cannot inject FTS syntax.

## Usage

```toml
[dependencies]
tpt-lean-index = "0.1"
```

```
tpt-lean index scan
tpt-lean index refresh
tpt-lean index status
tpt-lean index clear
tpt-lean index sidecar --exe path/to/tpt_sidecar   # or set TPT_LEAN_SIDECAR
```

`index scan` builds the index from scratch; `index refresh` updates it incrementally.
`index sidecar` runs the Lean sidecar for each module that has no current declarations and
stores the results. Modules must be built first (`lake build`), because Lean imports them from
their build outputs; modules that fail are reported and the rest are still stored.

## Limits

- Scans skip the directory names above and honour `.gitignore`; other ignore files are not read.
- Invalidation is by content hash plus import closure. A change in an imported project module
  invalidates the sidecar data of everything that imports it, transitively. Imports outside the
  project (for example `Init` or Mathlib) are not hashed; a change in those is caught only by the
  toolchain key, so bump the toolchain or run `index clear` after changing dependency versions.
- Schema migrations run one version at a time, each in its own transaction. A database from a
  newer tpt-lean is refused; older ones upgrade in place.
- Docstrings and dependency edges are stored inside the main tables rather than in separate
  tables, as planned in `todo.md`.
- Only declarations of modules the sidecar could import are stored. Goals and per-project
  symbol tables from Lean are not stored.

## Dependencies

| Crate | Purpose |
| --- | --- |
| `tpt-lean-core` | configuration, paths, errors |
| `tpt-lean-syntax` | parsing files into symbols and imports |
| `rusqlite` (bundled SQLite, FTS5) | storage |

## Testing

```
cargo test -p tpt-lean-index
```

The crate has unit tests for scanning, refresh, and search queries.

## License

Licensed under either of [MIT](https://opensource.org/licenses/MIT) or
[Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0), at your option.
