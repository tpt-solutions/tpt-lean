# tpt-lean-lsp

A Language Server Protocol proxy for Lean 4, part of
[tpt-lean](https://github.com/tpt-solutions/tpt-lean).

Editors connect to this proxy over stdin and stdout. The proxy forwards most requests to Lean's
own language server (`lake serve` or `lean --server`) and answers a few itself, using data that
tpt-lean already has: cached document symbols from the syntax layer, workspace symbols from the
project index, and hover goals from Lean.

Lean's responses are forwarded unchanged. The proxy adds speed and convenience; it does not
second-guess Lean.

## Status

Early development (`0.1.0`, not yet published). The proxy runs and handles the requests listed
below. Editor setup instructions are not written yet.

## Requests the proxy answers itself

- `initialize`, `shutdown`, `exit`
- `textDocument/documentSymbol`: built from `tpt-lean-syntax`, so it is available before Lean has
  finished elaborating the file.
- `workspace/symbol`: read from the project index at `.tpt-lean/index.sqlite`, when that file
  exists. The proxy only reads the index; it never writes it.
- `textDocument/hover`: the goal at the position, from Lean's `$/lean/plainGoal`, returned as
  Markdown.

## Forwarded to Lean

Any other request is forwarded, with the document URI mapped to the one Lean knows. Examples
include `$/lean/plainTermGoal`. Definition and code-action requests are forwarded but not tested
through the proxy.

## Notifications

- `textDocument/didOpen`, `didChange` (full text), and `didClose` update the session. When a file
  is reopened with unchanged text, cached diagnostics are published again immediately.
- Lean's `publishDiagnostics` are forwarded whenever the set changes.
- A failure to start or sync Lean is reported to the editor with `window/logMessage`.
- `$/cancelRequest` is handled on the reader thread, so cancellation works while the proxy is
  waiting for Lean.

## Usage

```
tpt-lean lsp
```

The command runs the proxy on stdin and stdout until the editor sends `exit`. Point your editor's
Lean language client at this command in place of `lake serve`. Run it from inside a Lake project.

```toml
[dependencies]
tpt-lean-lsp = "0.1"
```

`run_stdio(project_root)` is the library entry point.

## Limits

- Definition and code-action responses have no local implementation; they are only forwarded.
- Import suggestions, lemma search, and explain-error code actions are not offered yet.
- Progress reporting is not implemented.
- Document sync is full-text only; incremental sync is not implemented.

Protocol details are in the repository's `docs/lsp.md`.

## Dependencies

| Crate | Purpose |
| --- | --- |
| `tpt-lean-core` | configuration and paths |
| `tpt-lean-adapter` | the client that talks to Lean's server |
| `tpt-lean-syntax` | document symbols |
| `tpt-lean-index` | workspace symbols |
| `serde_json` | JSON-RPC messages |

## Testing

```
cargo test -p tpt-lean-lsp
```

The crate has unit tests. `tests/proxy_lean.rs` runs the proxy against a real Lean server and
needs Lean installed.

## License

Licensed under either of [MIT](https://opensource.org/licenses/MIT) or
[Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0), at your option.
