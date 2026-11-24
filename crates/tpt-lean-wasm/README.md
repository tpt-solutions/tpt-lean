# tpt-lean-wasm

Restricted, browser-safe WebAssembly bindings for [tpt-lean](https://github.com/tpt-solutions/tpt-lean),
a Rust toolbox for Lean 4.

The bindings expose pure computation over strings and caller-supplied bytes: tokenizing, CST
output, formatting, and searching a prebuilt index. They run in browsers and in Node.js.

This crate is not published to crates.io (`publish = false`). It is distributed as an npm package
built from `npm/` in the repository.

## What is excluded, on purpose

- process spawning (no Lean or Lake),
- native filesystem access,
- SQLite indexes (`tpt-lean-index` is not a dependency),
- profiling and build orchestration.

Lean remains the trusted proof checker. The WASM API is for editor features and quick lookups in
the browser, not for checking proofs.

## Status

Early development. The tokenizer, CST output, formatter, and search over a prebuilt index work and
are tested on native and under Node with `wasm-bindgen-test`.

## JavaScript API

| Export | Returns | Purpose |
| --- | --- | --- |
| `version()` | string | crate version |
| `tokenize(src)` | JSON string | token stream |
| `formatText(src)` | JSON string | formatted text and change status |
| `cstTree(src)` | JSON string | lossless concrete syntax tree |
| `class SearchIndex` | object | load an index with `new SearchIndex(bytes)`; throws on invalid bytes |
| `searchSymbols(...)` | JSON string | name search over a `SearchIndex` |
| `searchDocs(...)` | JSON string | docstring search over a `SearchIndex` |

`SearchIndex` exposes `len`, the number of entries. Parsing the index bytes happens once, when the
object is created, so reuse one `SearchIndex` across queries.

Exact result shapes are in the `text` and `search` modules and in the repository's `docs/wasm.md`.

## Index format

The index is a binary blob with format version `INDEX_VERSION` (1). The layout and ranking rules
are described in the repository's `docs/wasm.md`. This crate reads the blob; it does not build
one.

## Limits

- Source inputs above 8 MiB (`MAX_SOURCE_BYTES`) are rejected with `ok: false`.
- Search results are capped at `MAX_LIMIT`; the default is `DEFAULT_LIMIT`.
- The API has no access to a project, so it cannot resolve imports or check declarations.

## Building

The wasm-bindgen version is pinned to `=0.2.126` to match the JavaScript glue in `npm/pkg/`. See
[`npm/README.md`](https://github.com/tpt-solutions/tpt-lean/blob/master/crates/tpt-lean-wasm/npm/README.md)
for build and packaging steps.

```
cargo test -p tpt-lean-wasm
```

Native tests cover the pure-logic functions. The wasm-bindgen tests run on Node with
`wasm-bindgen-test-runner`.

## Dependencies

| Crate | Purpose |
| --- | --- |
| `tpt-lean-syntax` | tokens and CST |
| `tpt-lean-fmt` | formatting |
| `serde`, `serde_json` | JSON results |
| `wasm-bindgen` (pinned) | JavaScript bindings |

## License

Licensed under either of [MIT](https://opensource.org/licenses/MIT) or
[Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0), at your option.
