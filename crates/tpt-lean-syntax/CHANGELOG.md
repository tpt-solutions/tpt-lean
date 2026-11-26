# Changelog

All notable changes to `tpt-lean-syntax` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

Initial development version `0.1.0`. Not yet published; see [PUBLISHING.md](../../PUBLISHING.md).

### Added
- Lexer with keywords, namespaced and guillemet identifiers, numbers, strings, Unicode
  operators, nested block comments, and doc comments.
- Source spans (`TextRange`) and line/column mapping in UTF-8 and UTF-16.
- Lossless concrete syntax tree (`SyntaxNode`, `SyntaxElement`, `Token`) that preserves
  comments and whitespace.
- Import extraction.
- Declaration boundary detection for the common Lean declaration keywords.
- Lightweight symbol extraction (name, kind, namespace, range, docstring).
- `by` block detection (`TacticBlock`).
- Indentation tracking.
- Uncertainty markers (`Uncertain`) for parses that cannot be trusted.
- Golden test and deterministic round-trip test.

### Known limitations
- Tactic-block detection covers `by` only; `·` and `case` are not detected.
- The tree is shallow and is not a full Lean grammar.
