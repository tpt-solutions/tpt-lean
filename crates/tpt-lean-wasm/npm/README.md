# tpt-lean-wasm (npm package, not published)

Restricted, browser-safe WebAssembly build of tpt-lean. It exposes tokenizing, formatting,
a CST viewer and search over a prebuilt index. It cannot spawn processes, touch the native
filesystem, or use SQLite. See [docs/wasm.md](../../../docs/wasm.md) for the full API.

## Build

Requirements: Rust with the `wasm32-unknown-unknown` target, and `wasm-bindgen-cli` **0.2.126**
(must match the `wasm-bindgen` crate version exactly).

```sh
cd crates/tpt-lean-wasm/npm
npm run build      # cargo build --release for wasm32, then wasm-bindgen --target nodejs
npm run example    # runs examples/node-example.js against pkg/
```

`pkg/` is generated and git-ignored. For browsers, run `wasm-bindgen --target web` instead
and load the module with its default `init()` export.

## Node usage

```js
const wasm = require("./pkg/tpt_lean_wasm.js");

JSON.parse(wasm.formatText("def  x := 1  \n")); // { ok, output, changed } or { ok:false, error, uncertain }

const index = new wasm.SearchIndex(new Uint8Array(indexJsonBytes));
index.searchSymbols("nat.add", 10); // JSON string: { ok, results: [...] }
index.searchDocs("commutative", 10);
```

All functions return JSON strings. Parse them with `JSON.parse`. The only exception is
`new SearchIndex(bytes)`, which throws when the index is malformed or has an unsupported version.

## Tests

```sh
CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
  cargo test -p tpt-lean-wasm --target wasm32-unknown-unknown
```

This runs the wasm-bindgen tests under Node. Native pure-logic tests run with
`cargo test -p tpt-lean-wasm`.

## License

MIT OR Apache-2.0
