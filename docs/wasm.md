# WebAssembly API (`tpt-lean-wasm`)

`tpt-lean-wasm` is an optional, restricted, browser-safe build. It is pure computation over
strings and caller-supplied bytes. It does not spawn Lean or Lake, does not read or write the
native filesystem, and does not depend on `tpt-lean-index` or SQLite. Lean remains the source of
truth for semantics. Search results come from an index the caller provides.

## Exports

| JavaScript | Returns | Purpose |
| --- | --- | --- |
| `version()` | string | Crate version |
| `tokenize(src)` | JSON | Lossless token list |
| `formatText(src)` | JSON | Conservative formatter (same rules as `tpt-lean-fmt`) |
| `cstTree(src)` | JSON | Shallow concrete syntax tree |
| `new SearchIndex(bytes)` | object | Parse a version-1 index from a `Uint8Array`; throws on invalid input |
| `index.len`, `index.isEmpty` | number, boolean | Index size |
| `index.searchSymbols(query, limit?)` | JSON | Rank declaration names |
| `index.searchDocs(query, limit?)` | JSON | Rank docstrings |

Every JSON result has an `ok` boolean. Offsets are UTF-8 byte offsets into the input text,
not JavaScript string indices.

### tokenize

```json
{"ok":true,"tokens":[{"kind":"keyword","start":0,"end":6,"line":1,"text":"import"}]}
```

Kinds: `ident`, `keyword`, `number`, `string`, `char`, `whitespace`, `newline`, `lineComment`,
`blockComment`, `docComment`, `moduleDocComment`, `symbol`, `unknown`.

### formatText

Success: `{"ok":true,"output":"...","changed":true}`.

Refusal, when the syntax layer is uncertain (the formatter never writes in that case):
`{"ok":false,"error":"refusing to format: ...","uncertain":[{"start":12,"reason":"..."}]}`.

### cstTree

```json
{"ok":true,"certain":true,"tree":{"type":"node","kind":"file","start":0,"end":30,"children":[
  {"type":"node","kind":"import","start":0,"end":17,"children":[{"type":"token","kind":"keyword","start":0,"end":6,"text":"import"}]}
]}}
```

Node kinds: `file`, `import`, `scope`, `decl`, `command`, `trivia`.

### Search results

```json
{"ok":true,"results":[{"name":"Nat.add_comm","kind":"theorem","file":"Init/Nat.lean","line":10,"score":90,"docstring":"..."}]}
```

`limit` defaults to 20 and is capped at 200. `docstring` is truncated to 240 characters.
Empty queries return no results.

## Index format (version 1)

```json
{
  "version": 1,
  "entries": [
    {"name": "Nat.add_comm", "kind": "theorem", "file": "Init/Nat.lean", "line": 10,
     "docstring": "Addition on naturals is commutative."}
  ]
}
```

`docstring` is optional. Any other `version` is rejected. The index is prebuilt by the caller
(for example by a native tool); this crate never reads it from disk.

### Ranking

Matching is case-insensitive.

`searchSymbols` scores each entry on its name:

| Match | Score |
| --- | --- |
| Full name equals query | 100 |
| Last dotted component equals query | 90 |
| Full name starts with query | 70 |
| Last component starts with query | 60 |
| Full name contains query | 40 |

`searchDocs` scores on the docstring:

| Match | Score |
| --- | --- |
| Docstring contains the whole query | 50 |
| Docstring contains every whitespace-separated term | 30 |
| Full name also contains the query (bonus) | +20 |

Entries without a docstring never match `searchDocs`. Ties are ordered by name, file and line.

## Limits

- Source inputs above 8 MiB are rejected with `ok:false`.
- Index bytes are parsed once per `SearchIndex`. Reuse the object across queries.

## Building and testing

See `crates/tpt-lean-wasm/npm/README.md`. Native pure-logic tests run with
`cargo test -p tpt-lean-wasm`. The wasm-bindgen tests run on Node with
`wasm-bindgen-test-runner`.
