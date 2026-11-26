# Changelog

All notable changes to `tpt-lean-lsp` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

Initial development version `0.1.0`. Nothing has been published yet.

### Added
- stdio Language Server Protocol proxy in front of `lake serve` or `lean --server`.
- Local answers for `textDocument/documentSymbol`, `workspace/symbol` (from the index), and
  `textDocument/hover` (plain goals).
- Forwarding of all other requests with document URI mapping.
- Document lifecycle notifications, cached diagnostics, and forwarded `publishDiagnostics`.
- Cancellation handled on the reader thread.
- `tpt-lean lsp` command.

### Added (since the first draft)
- `textDocument/definition` from the index for exact names; `textDocument/codeAction` (import
  quick fix, lemma search, format, explain-error); `textDocument/formatting`.
- `tpt-lean.proofState` command; work-done progress from Lean's `$/lean/fileProgress`.
- A `didChange` with the text already open is ignored.

### Known limitations
- Full-text document sync only.
- Progress reporting is not tested in an editor.
