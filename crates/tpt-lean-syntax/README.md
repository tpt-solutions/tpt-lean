# tpt-lean-syntax

A lossless, lightweight syntax layer for Lean 4 source files, part of
[tpt-lean](https://github.com/tpt-solutions/tpt-lean).

This crate tokenizes Lean and builds a shallow concrete syntax tree that keeps every byte of
the input, including comments and whitespace. From that tree it extracts imports, declaration
boundaries, `by` blocks, and named symbols with their docstrings. Everything here is
text-level and fast enough to run on every keystroke or across a whole library.

It is **not** an elaborator. It does not know types, do not resolve names, and cannot say
whether a proof is valid. Semantic facts must come from Lean through `tpt-lean-adapter`.

## Status

Early development (`0.1.0`, not yet published). The lexer, lossless tree, import extraction,
declaration detection, symbol extraction, and uncertainty markers are implemented and tested.
Tactic-block detection covers `by` blocks only.

## What it provides

- **Lexer** (`lexer`): `lex` produces `Token`s with `TokenKind`. It recognizes keywords (see
  `KEYWORDS`), identifiers including namespaced and guillemet forms, numbers, strings, Unicode
  operators, line comments, nested block comments (`/- -/`), and doc comments.
- **Positions** (`position`): `TextRange`, `Position`, and `LineIndex`. Offsets map to lines and
  columns in both UTF-8 and UTF-16, so editor protocols that use UTF-16 columns work directly.
- **Concrete syntax tree** (`cst`): `SyntaxNode`, `SyntaxElement`, `SyntaxKind`, and `Token`.
  The tree is lossless: printing it reproduces the input exactly. Comments and whitespace are
  preserved as trivia.
- **Parsing** (`parse`, `parse_tree`, `build`): `parse` returns a `Parse` with the tree plus
  the extracted facts:
  - `Import` entries with their module names,
  - declaration boundaries (`theorem`, `lemma`, `def`, `instance`, `structure`, `inductive`,
    `class`, `abbrev`, `namespace`, `section`, and others listed in `DECL_KEYWORDS`),
  - `TacticBlock` entries for `by` blocks,
  - `Symbol` entries carrying the name, kind, fully qualified name when the namespace is known,
    source range, line, and docstring,
  - `Uncertain` markers for places where the parse may not be trustworthy.
- **Indentation** (`indentation_of_line`): indentation facts used by the formatter and the
  index.

## Uncertainty and safety

Parsing never fails hard on malformed input. Instead, `Parse` reports `Uncertain` markers
wherever it cannot be confident. `tpt-lean-fmt` refuses to write any file whose parse reports
uncertainty, so the formatter cannot damage code it does not understand.

## Usage

```toml
[dependencies]
tpt-lean-syntax = "0.1"
```

Typical pattern: call `parse` on the source text, then read `symbols`, `imports`, and
`uncertain` from the result. Use `LineIndex` to convert offsets into editor positions.

## Limits

- Shallow tree: items and declarations are recognized, but there is no full Lean grammar.
- Tactic blocks: only `by` blocks are detected. `·` bullets and `case` blocks are not.
- No name resolution, no types, no proof checking.

## Testing

```
cargo test -p tpt-lean-syntax
```

- Golden test: `tests/golden/basic/input.lean` is parsed and compared with
  `expected_symbols.txt`.
- Round-trip test: parse and print must reproduce the input. A deterministic pseudo-random
  test checks for panics and round-trip failures. It is not a `cargo-fuzz` target yet.

## Dependencies

None. The crate uses only the standard library, which keeps it small enough to embed in WASM
builds (see `tpt-lean-wasm`).

## License

Licensed under either of [MIT](https://opensource.org/licenses/MIT) or
[Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0), at your option.
