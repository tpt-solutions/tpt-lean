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
- `textDocument/definition`: when the project index has declarations whose full or short name is
  the dotted word under the cursor, the locations of those declarations (`workspace.rs`
  `definitions`). Otherwise the request is forwarded to Lean. The word is found in the editor's
  current text, so it works on unsaved edits.

Forwarded to Lean unchanged, with the document URI mapped to the one Lean knows:

- Any other request, for example `$/lean/plainTermGoal` or `textDocument/definition`. These are
  not tested through the proxy.

Code actions (`textDocument/codeAction`), answered by the proxy:

- Quick fix `Import M`: for an `unknown identifier 'x'` or `unknown constant 'x'` diagnostic, up to
  three project or dependency modules that declare `x` exactly, inserted after the last `import`
  line. Modules the file already imports are not offered.
- `Search lemmas for this goal` (command `tpt-lean.searchLemmas`): lemma candidates from the index
  for the goal at the position.
- `Explain this error` (command `tpt-lean.explainError`), offered when the request has
  diagnostics: the error at the position and the goals there.
- `Format document`: a source action whose edit replaces the whole document with the output of
  `tpt-lean fmt`. The same edit is returned by `textDocument/formatting` (advertised as
  `documentFormattingProvider`). The formatter refuses uncertain syntax and tabs; then no edit is
  returned and the reason is logged. Nothing is written to disk. Lean's check that a format keeps
  the file valid (`fmt --verify`) is not run here.
- Lean's own code actions are forwarded and appended. If Lean does not answer, the local actions
  are still returned.

`workspace/executeCommand` runs the two commands. Each takes one argument, `{uri, line, character}`.
Results are returned as the command result. Clients that show nothing for command results will not
show them; the lemma search also logs its count.

Notifications:

- `didOpen`, `didChange` (full text), `didClose` update the session. `didOpen` re-publishes the
  cached diagnostics when the text is unchanged since they were computed.
- Lean's `publishDiagnostics` are forwarded whenever the set changes.
- A failure to start or sync Lean is reported with `window/logMessage`.

Cancellation: `$/cancelRequest` is handled on the reader thread, so it works while the proxy is
waiting on Lean.

Not done: progress reporting, incremental sync, and running the Lean check of formatting edits. The index-backed definition answer is matched by exact name, so it does not
resolve names through `open` or namespaces; those fall through to Lean. Editor setup is in
[editors.md](editors.md).
