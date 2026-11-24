# Dependency policy

tpt-lean is licensed `MIT OR Apache-2.0`. Its dependency tree must stay legally simple.

## Allowed licenses

MIT, BSD-2-Clause, BSD-3-Clause, ISC, Zlib, Unlicense, CC0-1.0, Unicode-3.0.

A crate whose license is an OR-expression (for example `MIT OR Apache-2.0`) is accepted when
one alternative is on the list. A crate licensed only as Apache-2.0 is rejected. This is
checked by `cargo deny check licenses` using `deny.toml`.

## Checks

- `cargo deny check licenses bans sources advisories`: run in CI.
- `cargo audit`: run in CI.
- `scripts/license-report.sh`: produces the license report used for each release.

## Current direct dependencies

| Crate | Why | License |
| --- | --- | --- |
| serde, serde_json | JSON schemas | MIT OR Apache-2.0 |
| toml | configuration files | MIT OR Apache-2.0 |
| sha2 | content hashes and cache keys | MIT OR Apache-2.0 |
| rusqlite (bundled SQLite) | index storage with FTS5 | MIT |
| tracing, tracing-subscriber | logging | MIT |

Hashing uses SHA-256 rather than BLAKE3 for now. Both are acceptable; SHA-256 avoids one
more dependency.

## Verified rejection

A probe crate depending on `aws-smithy-types` (Apache-2.0 only) fails
`cargo deny check licenses` with `license is not explicitly allowed`. This confirms the
policy rejects Apache-only crates.

## Advisories

`rusqlite` 0.32 pulled in a `libsqlite3-sys` affected by RUSTSEC-2026-0459. The project now
uses `rusqlite` 0.43, which resolves to a patched `libsqlite3-sys`.
