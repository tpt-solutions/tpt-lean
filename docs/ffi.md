# tpt-lean C ABI (`tpt-lean-ffi`)

The contract is `crates/tpt-lean-ffi/include/tpt_lean.h` (ABI version 1). The Rust side is
`crates/tpt-lean-ffi/src/lib.rs`. `tests/abi.rs` fails if the header and the exports disagree.

Build: `cargo build -p tpt-lean-ffi` produces `tpt_lean_ffi.dll` / `libtpt_lean_ffi.so` /
`libtpt_lean_ffi.dylib` (cdylib), a static library, and on Windows the import library
`tpt_lean_ffi.dll.lib`.

## Rules

- Opaque handles; plain C types; explicit lengths; no Rust types cross the boundary.
- Every export runs under `catch_unwind`. A panic becomes `TPTL_ERR_INTERNAL` with the text
  `internal panic`, or 0 for `tptl_abi_version`. No unwinding crosses the boundary.
- Status codes are fixed: `0 OK`, `1 INVALID_ARGUMENT`, `2 IO`, `3 LEAN_NOT_FOUND`, `4 INDEX`,
  `5 INTERNAL`. Values never change.
- Strings returned through out parameters are owned by the caller and freed with
  `tptl_string_free`. Null is accepted by every `*_free` function.
- Out parameters are written only on success. On error they are left untouched (null).

## Exports

| Function | Purpose |
| --- | --- |
| `tptl_abi_version` | ABI version (1). |
| `tptl_context_new` / `tptl_context_free` | Context handle; holds the last error text. |
| `tptl_last_error` | Last error on a context. Valid until the next call on that context. |
| `tptl_index_open` / `tptl_index_free` | Open the index of the project containing a path. |
| `tptl_search_query` | Search an index. Limit 0 means 10; values above 1000 are clamped. |
| `tptl_search_results_to_json` / `tptl_search_results_free` | Result JSON, valid until freed. |
| `tptl_format_text` | Format Lean source; refuses uncertain syntax. |
| `tptl_syntax_parse` | JSON: `source_len`, `tokens`, `imports`, `symbols`, `uncertain`. Byte offsets. |
| `tptl_diagnostics_parse` | Parse Lean CLI output into `{"diagnostics": [...]}`. |
| `tptl_cache_query` | Look up `<global cache>/entries/<digest>.json`. Read-only. |
| `tptl_string_free` | Free any owned string. |

Functions that take a context report error text on it. `tptl_search_query` takes no context, so
its failures are visible only through the status code.

`tptl_cache_query` validates the digest (64 lowercase hex characters, the output of
`CacheKey::digest`). A missing entry is `found: false`, not an error. Nothing in this library
writes cache entries yet; the lookup is defined so callers can rely on the shape.

## Thread safety

| Handle | Behaviour |
| --- | --- |
| `TptlContext` | Thread-safe. |
| `TptlIndex` | Thread-safe for reads. Calls are serialized by an internal mutex, so concurrent searches are safe but do not run in parallel. |
| `TptlSearchResults` | Immutable after creation; safe to read from any thread until freed. |

`tests/abi.rs::concurrent_reads_on_one_index_handle` runs 8 threads x 25 searches on one handle.

## Tests

- Rust: `cargo test -p tpt-lean-ffi` (unit tests, the invalid-argument matrix, concurrent reads,
  limit clamping, header/export consistency, status code values).
- Python ctypes: `python -m unittest discover -s crates/tpt-lean-ffi/tests/ffi -v` after
  `cargo build -p tpt-lean-ffi`. Covers ABI version, format and free, error text, null frees,
  syntax and diagnostics parsing, cache query, an index opened on an empty project, and that
  every header declaration resolves in the library.
- C: `crates/tpt-lean-ffi/tests/c/harness.c`. `build_msvc.bat` (MSVC; verified here) or
  `build.sh` (gcc/clang; not run, no such compiler was available).
- Go, without cgo: `crates/tpt-lean-ffi/tests/go` loads the Windows DLL with `syscall.LoadDLL` and
  checks the ABI version, context lifecycle, formatting, error reporting, parsing and index open.
  Run `cargo build -p tpt-lean-ffi` and then `go test ./...` in that directory. Go's `vet` reports
  `possible misuse of unsafe.Pointer` for the one `uintptr` to pointer conversion that reads C
  strings; that conversion is the usual way to read foreign memory without cgo.
- Go with cgo: not written. This machine has no C compiler, so a cgo package could not be built
  or run here.

## Not in ABI 1

Build orchestration, profiling, goal inspection, async callbacks and event streams are not
exported. Adding exports is compatible; changing or removing one needs an ABI bump.
