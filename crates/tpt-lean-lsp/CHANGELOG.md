# Changelog

All notable changes to `tpt-lean-lsp` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

Initial development version `0.1.0`. Not yet published; see [PUBLISHING.md](../../PUBLISHING.md).

### Added
- stdio Language Server Protocol proxy in front of `lake serve` or `lean --server`.
- Local answers for `textDocument/documentSymbol`, `workspace/symbol` (from the index), and
  `textDocument/hover` (plain goals).
- `textDocument/definition` from the index for exact names; otherwise forwarded to Lean.
- `textDocument/codeAction`: an `Import M` quick fix for unknown identifiers, and lemma search,
  format and explain-error actions. Commands `tpt-lean.searchLemmas` and `tpt-lean.explainError`
  are available through `workspace/executeCommand`.
- `textDocument/formatting` and a `Format document` action, using `tpt-lean fmt`.
- `tpt-lean.proofState` command: goals at a position, split into hypotheses and target.
- Work-done progress from Lean's `$/lean/fileProgress`.
- Forwarding of all other requests, with document URI mapping.
- Document lifecycle notifications, cached diagnostics, and forwarded `publishDiagnostics`.
- Cancellation handled on the reader thread.
- `tpt-lean lsp` command, used by the VS Code extension in `editors/vscode/`.

### Changed
- A `didChange` or `didOpen` with the text already open is ignored.

### Known limitations
- Full-text document sync only.
- Progress reporting and the VS Code extension have not been run in an editor.
