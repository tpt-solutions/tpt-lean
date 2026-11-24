# tpt-lean-cli

The command-line interface for [tpt-lean](https://github.com/tpt-solutions/tpt-lean), a fast
Rust toolbox for Lean 4. This crate ships two binaries with the same behavior:

- `tpt-lean`
- `tlean` (short alias)

It wires the library crates in the workspace to commands. It contains almost no logic of its
own; the behavior described here comes from the components it calls.

## Installing

From a checkout of the repository:

```
cargo install --path crates/tpt-lean-cli
```

Requires Rust 1.82 or newer. Lean and Lake are needed for the commands that run them
(`check`, `goals`, `explain`, `context-pack`, `build`, `profile`, `ci`, `bisect`, `lsp`).

## Commands

| Command | What it does |
| --- | --- |
| `init <dir> [--mathlib]` | create a starter Lean project |
| `doctor` | check Lean, Lake, project, configuration, index, cache, and server socket |
| `config show` | print the effective configuration |
| `fmt [--check \| --diff] [paths]` | conservative, idempotent formatter |
| `index scan \| refresh \| status \| clear` | SQLite symbol index |
| `search <query> [--namespace NS] [--near TERM] [--limit N]` | name, prefix, and docstring search |
| `check [file]` | `lake build`, or `lean <file>` |
| `goals FILE:LINE:COL` | proof goal at a position |
| `explain FILE:LINE:COL` or `explain --last-error` | explain a diagnostic |
| `context-pack FILE:LINE:COL [--max-tokens N]` | context bundle for a position |
| `build [target] [--watch] [...]` | `lake build` with an event stream and cache |
| `cache status \| prune` | global cache location, size, and pruning |
| `profile FILE [--slowest N]` or `profile --compare A B` | timing reports |
| `imports --unused \| --suggest \| --minimize` | import analysis |
| `docgen [--out DIR]` | static HTML documentation |
| `ci [--sarif]` | build and report failures, optionally as SARIF |
| `bisect --good REV --bad REV` | find the first failing commit |
| `lsp` | Lean language-server proxy over stdin and stdout |
| `mcp stdio \| http` | MCP server |
| `server start \| status \| stop` | local JSON-RPC API server |
| `clean` | run `lake clean` for the project |

Not yet implemented (these exit with code 10): `profile --flamegraph` and `fmt --verify`.

## Global flags

- `--json`: print one JSON envelope object per command.
- `--ndjson`: print one record per line, for list-shaped results.
- `--project <dir>`: use this directory as the project root.
- `--set section.key=value`: override a configuration value for one run.
- `--verbose`, `--quiet`: control progress and notes on stderr.

Human output is the default. Positions use `FILE:LINE:COL`.

## Configuration

Settings are resolved in this order, with later sources overriding earlier ones:

1. built-in defaults,
2. the project file `tpt-lean.toml`,
3. `.tpt-lean/config.toml`,
4. environment variables,
5. command-line flags, including `--set`.

`tpt-lean config show` prints the result.

## Exit codes

Exit codes come from `tpt-lean-core`'s `ErrorCode::exit_code`. Commands that are planned but not
built fail with `not_implemented` and exit code 10.

## Output contract

With `--json`, errors are also JSON: an error envelope with a stable `code` and a message. Scripts
can branch on the code rather than on the text. See `docs/cli.md` for the full contract.

## Testing

```
cargo test -p tpt-lean-cli
```

The crate has unit tests.

## Related documentation

- [CLI reference](https://github.com/tpt-solutions/tpt-lean/blob/master/docs/cli.md)

## License

Licensed under either of [MIT](https://opensource.org/licenses/MIT) or
[Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0), at your option.
