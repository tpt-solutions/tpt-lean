# tpt-lean-fmt

A conservative, syntax-preserving, idempotent formatter for Lean 4, part of
[tpt-lean](https://github.com/tpt-solutions/tpt-lean).

The formatter only makes changes it can justify from the source text. It never rewrites
tokens, never touches the contents of comments or string literals, and refuses to write a file
whose syntax tree is invalid or uncertain. Running it twice gives the same result as running it
once.

It is deliberately not a pretty-printer. It will not reflow long lines, change layout of
expressions, or reorder declarations. Lean is the trusted checker; the formatter is for keeping
files tidy without risk.

## Status

Early development (`0.1.0`, not yet published). The rules below are implemented and covered by
golden and idempotency tests. Verification against Lean (`fmt --verify`) is not implemented
yet; it needs `tpt-lean-adapter`.

## Rules

All rules are idempotent. Comment and string contents are never changed.

- Strip trailing whitespace; blank out lines that contain only whitespace.
- End non-empty files with exactly one newline.
- Collapse runs of spaces between tokens on one line to a single space.
- Remove spaces before `,`, `)`, `]`, and `}`.
- Use a single space around `:=`, `=>`, `->`, `<-`, `→`, `←`, and `↔`.
- Sort and deduplicate the leading contiguous block of `import` lines.
- Indent the body of a `by` block that ends its line to at least two columns deeper than the
  line containing `by`.
- Tab indentation is refused rather than converted.

## Safety

Before writing, the formatter checks the parse from `tpt-lean-syntax`. If the parse reports any
uncertainty, formatting fails with an error and the file is left unchanged. This is the main
guard against damaging code the tokenizer does not fully understand.

## Public API

- `format_text(source)` returns `Formatted`: the new text and whether it changed.
- `diff(original, formatted)` returns `Hunk` values; `render_hunk` prints one in unified style.
- `range_of(...)` maps a hunk to line ranges in the original file.
- `FmtError` describes refusals (for example, an uncertain parse).

## Usage

```
tpt-lean fmt                 # format the project in place
tpt-lean fmt --check         # exit non-zero if anything would change
tpt-lean fmt --diff          # print the changes without writing
tpt-lean fmt path/to/File.lean
```

Output modes `--json` and `--ndjson` are available on every command.

```toml
[dependencies]
tpt-lean-fmt = "0.1"
```

## Limits

- `fmt --verify` (checking the formatted file with Lean) is not implemented.
- The benchmark target (a 5,000-line file in under 100 ms) has not been measured.
- Only the rules listed above are applied; there is no general layout engine.

## Dependencies

| Crate | Purpose |
| --- | --- |
| `tpt-lean-syntax` | parsing and uncertainty detection |

## Testing

```
cargo test -p tpt-lean-fmt
```

- Golden test: `tests/golden/basic/input.lean` must format to `expected_formatted.lean`.
- Idempotency tests: formatting a formatted file must change nothing.

## License

Licensed under either of [MIT](https://opensource.org/licenses/MIT) or
[Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0), at your option.
