# Changelog

All notable changes to `tpt-lean-ffi` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

Initial development version `0.1.0`. Nothing has been published yet.

### Added
- C ABI built as `cdylib`, `staticlib`, and `rlib`.
- Hand-written header `include/tpt_lean.h` as the ABI contract.
- `tptl_abi_version()`, separate from the crate version.
- Context, index, search, syntax, formatting, diagnostic, and cache-query functions.
- Stable status codes (`TptlStatus`).
- `catch_unwind` on every export.
- Thread-safety notes for each handle type.
- Tests: header agreement (every exported function is declared, and every declared function is
  exported), a C harness, and a Python `ctypes` test.

### Known limitations
- No Go (cgo) tests.
- Header is hand-written; generation is optional.
- Not yet verified on every platform in CI.
