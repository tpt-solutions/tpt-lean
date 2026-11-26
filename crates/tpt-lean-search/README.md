# tpt-lean-search

Search over the local [tpt-lean](https://github.com/tpt-solutions/tpt-lean) index. Supports
exact, prefix, and namespace-aware name search, docstring full-text search, namespace filters,
`--near` co-location filtering, imported-module preference, type-shaped search, and deterministic
ranking.

This crate only reads. It takes an open `tpt-lean-index` and returns ranked hits. Results are
search hints, not proofs; check them with Lean before relying on them.

## Status

Early development (`0.1.0`, not yet published). Name search, docstring search, the namespace,
`--near` and `--from` filters, type-shaped search, and ranking are implemented. Some ranking
factors from the specification are still missing (see Limits).

## Features

- **Name matching**, in order of strength:
  1. exact full name,
  2. exact short name,
  3. prefix of the full name,
  4. prefix of the short name.
- **Docstring relevance** through the FTS5 table in `tpt-lean-index`.
- **Namespace filter**: keeps only symbols whose namespace equals, or lies under, a given prefix.
- **`--near` filter**: keeps only symbols whose file also declares a symbol matching each of
  the listed terms, so you can find a lemma defined next to other related lemmas.
- **Imported-module preference** (`--from FILE` in the CLI): symbols from the modules that FILE
  imports rank higher.
- **Type-shaped search** (`--type PATTERN`): matches declaration types from the Lean sidecar.
- **Goal-based search** (`--goal-at FILE:LINE:COL`): searches for names that fit the goal at a
  position. It needs Lean, through `tpt-lean-adapter`.
- **Kind and package weighting**: theorems and definitions are weighted by kind, and project
  symbols rank above dependency symbols.

Scores are deterministic and normalized to `0.0..=1.0`. The same index and query always produce
the same order.

## Public API

- `search(index, query, options)` runs a search and returns `Hit` values.
- `SearchOptions` carries the namespace prefix, `--near` terms, imported modules, and result
  limit.
- `Hit` carries the matched symbol and its normalized score.

## Usage

```
tpt-lean search <query> [--from FILE] [--namespace NS] [--near TERM] [--limit N]
tpt-lean search --type PATTERN
tpt-lean search --goal-at FILE:LINE:COL
```

Output can be a table, JSON (`--json`), or NDJSON (`--ndjson`).

```toml
[dependencies]
tpt-lean-search = "0.1"
```

## Limits

- Ranking does not yet include recency or dependency distance.
- Type-shaped search (`--type`) depends on the sidecar data from `tpt-lean index sidecar`.
- Measured on 5,000 symbols (release build, `tests/bench.rs`): search p50 0.99 ms and p95 4.0 ms.
  The warm targets (p50 under 10 ms, p95 under 50 ms) are met on that set.

## Dependencies

| Crate | Purpose |
| --- | --- |
| `tpt-lean-core` | shared types and errors |
| `tpt-lean-index` | the index being searched |

## Testing

```
cargo test -p tpt-lean-search
cargo test -p tpt-lean-search --release --test bench -- --ignored
```

The crate has unit tests for search behavior. The benchmark is ignored by default.

## License

Licensed under either of [MIT](https://opensource.org/licenses/MIT) or
[Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0), at your option.
