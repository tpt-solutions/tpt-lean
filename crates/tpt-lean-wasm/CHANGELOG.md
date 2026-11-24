# Changelog

All notable changes to `tpt-lean-wasm` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

Initial development version `0.1.0`. Nothing has been published yet. This crate is not published
to crates.io (`publish = false`); it is distributed through npm.

### Added
- WebAssembly exports: `version`, `tokenize`, `formatText`, `cstTree`.
- `SearchIndex` class with `searchSymbols` and `searchDocs` over a prebuilt index (format
  version 1).
- Input size limit of 8 MiB.
- Native and `wasm-bindgen-test` test suites.
- npm package scaffolding in `npm/`, with a pinned `wasm-bindgen` version.

### Known limitations
- No access to projects, the filesystem, or Lean; the API is pure computation only.
