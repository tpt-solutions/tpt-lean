# tpt-lean-ffi

A stable C ABI for [tpt-lean](https://github.com/tpt-solutions/tpt-lean). This crate builds as a
shared library (`cdylib`), a static library (`staticlib`), and an `rlib`, so C, Python, Go, and
other languages can call tpt-lean's search, syntax, formatting, index, and diagnostic functions
without running the CLI.

The public contract is the hand-written header `include/tpt_lean.h`. Its declarations and the
exported symbols must stay in sync; change both together.

This is the only crate in the workspace that contains `unsafe` code. The rest of the workspace
forbids it.

## Status

Early development (`0.1.0`, not yet published). The exported functions below are implemented
and tested from Rust, C, and Python. Go is tested on Windows without cgo (see Testing).

## Exported functions

| Area | Functions |
| --- | --- |
| ABI | `tptl_abi_version` |
| Context | `tptl_context_new`, `tptl_context_free`, `tptl_last_error` |
| Index | `tptl_index_open`, `tptl_index_free` |
| Search | `tptl_search_query`, `tptl_search_results_to_json`, `tptl_search_results_free` |
| Cache | `tptl_cache_query` |
| Syntax | `tptl_syntax_parse` |
| Formatting | `tptl_format_text` |
| Diagnostics | `tptl_diagnostics_parse` |
| Memory | `tptl_string_free` |

Status codes are the `TptlStatus` enum. Their values are part of the ABI.

## ABI rules

- **Opaque handles.** Callers receive pointers to `TptlContext`, `TptlIndex`, and
  `TptlSearchResults`, and never see their layout.
- **Plain C types.** Strings are UTF-8 with explicit lengths where needed. Nothing is passed as a
  Rust type.
- **Explicit ownership.** Each allocation has a matching free function
  (`tptl_context_free`, `tptl_index_free`, `tptl_search_results_free`, `tptl_string_free`).
- **Versioned ABI.** `tptl_abi_version()` returns `ABI_VERSION`, which is separate from the crate
  version. Breaking ABI changes bump it.
- **No panics across the boundary.** Every export runs under `catch_unwind`; a panic becomes a
  status code and an error message.
- **Bounded input.** `tptl_search_query` clamps results to `MAX_SEARCH_LIMIT`.

## Thread safety

- `TptlContext` is safe to share between threads.
- `TptlIndex` serializes its calls internally (its SQLite connection is behind a mutex). Reads
  from several threads are safe but do not run in parallel.
- `TptlSearchResults` is immutable after creation and can be read from any thread.
- The pointer returned by `tptl_last_error` is valid until the next call on the same context.

## Building

```
cargo build -p tpt-lean-ffi --release
```

This produces `tpt_lean_ffi` as a shared library (`.so`, `.dylib`, or `.dll`) and a static
library in `target/release`.

## Testing

```
cargo test -p tpt-lean-ffi
```

- `tests/abi.rs` exercises the ABI from Rust.
- `tests/header.rs` checks that the header declares exactly the `tptl_*` functions exported from
  `src/lib.rs`.
- `tests/c/harness.c` is a C program that calls the library. Build it with `tests/c/build.sh`
  or, on Windows with MSVC, `tests/c/build_msvc.bat`.
- `tests/ffi/test_ffi.py` drives the library through Python's `ctypes`.
- `tests/go` (Windows): six Go tests load the DLL with `syscall.LoadDLL`. They do not use cgo,
  and they pass against the debug build.

## Limits

- The Go tests run only on Windows, because they load the Windows DLL.
- The header is maintained by hand; generation is optional and not a blocker.
- The build and tests have not yet been run on every target in CI (Windows, Linux, and macOS).
- Async callbacks and event streams are not part of this ABI.

## Dependencies

Workspace crates `tpt-lean-core`, `tpt-lean-index`, `tpt-lean-search`, `tpt-lean-fmt`,
`tpt-lean-syntax`, and `tpt-lean-adapter`, plus `serde_json`.

## License

Licensed under either of [MIT](https://opensource.org/licenses/MIT) or
[Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0), at your option.
