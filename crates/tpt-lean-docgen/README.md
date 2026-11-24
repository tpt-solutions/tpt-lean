# tpt-lean-docgen

A static documentation generator for Lean 4 projects, part of
[tpt-lean](https://github.com/tpt-solutions/tpt-lean).

It produces a folder of plain HTML that can be opened from disk or hosted anywhere. The site
includes a local search index, dark mode, a page for each source file with line anchors, a
namespace tree, an import graph, and a JSON export of the API.

Facts come from `tpt-lean-syntax`: declarations, namespaces, docstrings, and imports. No Lean
process runs and no network access is needed. Because the facts are syntactic, the docs show
what the source declares, not what Lean elaborates; types and other semantic details are not
included.

## Status

Early development (`0.1.0`, not yet published). Generation works and is covered by tests.

## Output

Written into the output directory:

- one HTML page per module, plus an index page,
- source pages with line anchors,
- a namespace tree and an import graph,
- `search-index.json` for the built-in client-side search,
- `api.json`, a machine-readable export.

Existing files with the same names are overwritten. Nothing else in the output directory is
removed.

## Public API

- `generate(project_root, out_dir, include_deps)` builds the whole site. When `include_deps` is
  true, the packages under `.lake/packages` are documented too.
- `collect` gathers the facts into a `Collected` value.
- `export_json` and `to_json` produce `api.json`.
- `write_site` writes the HTML for an already-collected model.
- `Summary` reports what was generated; it includes import edges whose targets are not
  documented modules.
- Model types: `Model`, `ModuleInfo`, `DeclInfo`, `ImportEdge`.
- `PROJECT_PACKAGE` names the project's own package in the output.

## Usage

```
tpt-lean docgen [--out DIR] [--open] [--json] [--include-deps]
```

The default output is `.tpt-lean/docs`. `--json` prints a machine-readable summary. The search
box needs the site served over HTTP; opening the files directly from disk is not enough for
search.

```toml
[dependencies]
tpt-lean-docgen = "0.1"
```

## Limits

- Syntactic facts only: no types, no elaborated statements, no cross-references resolved by
  Lean.
- Docstrings are taken from the source text as written.
- The generator does not check whether the project builds.

## Dependencies

| Crate | Purpose |
| --- | --- |
| `tpt-lean-core` | paths, errors |
| `tpt-lean-syntax` | declarations, docstrings, imports |
| `tpt-lean-index` | module and package layout |
| `serde`, `serde_json` | `api.json` |

## Testing

```
cargo test -p tpt-lean-docgen
```

The crate has unit tests.

## License

Licensed under either of [MIT](https://opensource.org/licenses/MIT) or
[Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0), at your option.
