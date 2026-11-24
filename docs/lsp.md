# LSP support

tpt-lean talks to Lean's own language server in two ways. Both use the same stdio client in
`tpt-lean-adapter`.

## Adapter client (`tpt-lean-adapter`)

- `LspClient` runs `lake serve` (inside a Lake project) or `lean --server`, performs the
  `initialize` handshake, and records `serverInfo.version` when the server reports it.
- Document sync is full-text: `sync_document` sends `didOpen` the first time and `didChange`
  with the whole text after that. The server re-elaborates the changed document.
- Goals: `plain_goals` (`$/lean/plainGoal`) and `plain_term_goal` (`$/lean/plainTermGoal`).
- Diagnostics: the latest `publishDiagnostics` per URI (`published_diagnostics`).
- Cancellation: `request_cancellable` returns within about 20 ms of the `CancelToken` being set,
  sends `$/cancelRequest` for the abandoned request, and returns an error for which
  `lsp::is_cancelled` is true.
- `LspSession` adds crash recovery. The server starts lazily. If it has exited, or a request
  fails with an I/O error, the session restarts it once, replays open documents at their latest
  versions, and retries the request once. Cancelled and timed-out requests are not retried.
  `restart_count()` reports how many restarts happened.

Not done: server version compatibility checks (the version is recorded but no feature is gated on
it), and testing against a previous stable Lean release.

## Proxy (`tpt-lean-lsp`)

`tpt-lean-lsp::run_stdio(project_root)` serves an editor over stdin and stdout.

Answered by the proxy:

- `initialize`, `shutdown`, `exit`
- `textDocument/documentSymbol`: from `tpt-lean-syntax`, available before Lean finishes
- `workspace/symbol`: from the project index at `.tpt-lean/index.sqlite`, when that file exists.
  The proxy only reads the index.
- `textDocument/hover`: the goal at the position, from `$/lean/plainGoal`, as Markdown

Forwarded to Lean unchanged, with the document URI mapped to the one Lean knows:

- Any other request, for example `$/lean/plainTermGoal` or `textDocument/definition`. These are
  not tested through the proxy.

Notifications:

- `didOpen`, `didChange` (full text), `didClose` update the session. `didOpen` re-publishes the
  cached diagnostics when the text is unchanged since they were computed.
- Lean's `publishDiagnostics` are forwarded whenever the set changes.
- A failure to start or sync Lean is reported with `window/logMessage`.

Cancellation: `$/cancelRequest` is handled on the reader thread, so it works while the proxy is
waiting on Lean.

Not done: definition and code-action responses (no local implementation), import suggestions,
lemma-search and explain-error code actions, progress reporting, and incremental sync. Editor
setup instructions are not written yet.
