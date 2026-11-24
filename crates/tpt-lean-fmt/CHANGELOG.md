# Changelog

All notable changes to `tpt-lean-fmt` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

Initial development version `0.1.0`. Nothing has been published yet.

### Added
- Whitespace normalization: trailing whitespace, whitespace-only lines, final newline.
- Spacing normalization around common tokens and before closing brackets and commas.
- Import block sorting and deduplication.
- Indentation of `by` block bodies.
- Refusal to write when the syntax tree is invalid or uncertain.
- Unified diff output and per-hunk line ranges.
- Golden and idempotency tests.

### Known limitations
- `fmt --verify` is not implemented (needs the Lean adapter).
- Tab indentation is refused, not converted.
