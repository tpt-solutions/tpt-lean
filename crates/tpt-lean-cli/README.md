# tpt-lean-cli

The command-line interface for [tpt-lean](https://github.com/tpt-solutions/tpt-lean), a fast
Rust toolbox for Lean 4. This crate ships two binaries with the same behavior:

- `tpt-lean`
- `tlean` (short alias)

It wires the library crates in the workspace to commands. It contains almost no logic of its
own; the behavior described here comes from the components it calls. The root
[README](../../README.md) has the same command table, and [docs/cli.md](../../docs/cli.md) lists
every flag and exit code.

## Installing

From a checkout of the repository:

```
cargo install --path crates/tpt-lean-cli
```

Requires Rust 1.82 or newer. Lean and Lake are needed for the commands that run them: `build`,
`check`, `goals`, `explain`, `context-pack`, `profile`, `ci`, `bisect`, `watch`, `lsp`,
`index sidecar`, `init --mathlib`, and `upgrade --apply`.

## Commands

| Command | What it does |
| --- | --- |
| `init <dir> [--mathlib] [--template NAME]` | create a starter Lean project (`--template mathlib` is an alias for `--mathlib`) |
| `doctor [--fix]` | check Lean, Lake, project, configuration, index, cache, and server socket; `--fix` rebuilds a missing or damaged index |
| `config show` | print the effective configuration |
| `fmt [--check \| --diff \| --verify] [paths]` | conservative, idempotent formatter (`--verify` is not implemented) |
| `imports [file] [--unused \| --cost \| --minimize FILE \| --suggest QUERY]` | import analysis |
| `search <query> [--from FILE] [--namespace NS] [--near TERM] [--limit N]` | name, prefix and docstring search; `--type PATTERN` and `--goal-at FILE:LINE:COL` search by declaration type or goal |
| `index scan \| refresh \| status \| clear \| sidecar` | SQLite symbol index (incremental); `sidecar` stores Lean declarations |
| `build [target] [--watch \| --plan \| --report \| --explain-slow]` | `lake build` through typed options, with an event stream and a cache |
| `check [file]` | `lake build`, or `lean <file>` |
| `watch [--once] [--interval-ms N]` | re-check changed files; one diagnostics and `sorry` summary per file |
| `sorry-report [paths] [--deny-sorry]` | `sorry` and `admit` per declaration; `--deny-sorry` exits 1 when any is found |
| `goals FILE:LINE:COL [--sidecar]` | goals at a position, from Lean's own info trees |
| `explain FILE:LINE:COL \| --last-error` | explain an error at a position |
| `context-pack FILE:LINE:COL [--max-tokens N]` | bounded context for a proof, for an assistant or a person |
| `profile FILE [--slowest N] [--imports] [--graph \| --compare A B]` | where Lean spends its time (`--flamegraph` is not implemented) |
| `docgen [--out DIR]` | static HTML documentation from the sources |
| `ci [--json \| --sarif]` | run the project checks as one CI job |
| `bisect --good REV --bad REV` | first failing git commit |
| `upgrade --to VERSION [--apply]` | plan a `lean-toolchain` change; `--apply` builds and restores the file on failure |
| `clean` | `lake clean` |
| `cache status \| prune \| export DIR \| import DIR` | the global build cache, and moving its records for CI artifacts |
| `server start [--port N] [--http PORT] \| status \| stop` | the local JSON-RPC API server |
| `mcp stdio \| http` | the Model Context Protocol server |
| `lsp` | the language-server proxy for editors |
| `setup [--editor NAME]... [--mcp NAME]... [--dry-run]` | write editor and MCP config for the project, showing a diff first |
| `completions bash \| zsh \| fish \| powershell` | print a shell completion script for `tpt-lean` and `tlean` |

Not yet implemented (these exit with code 10): `profile --flamegraph` and `fmt --verify`.

Other documentation: [pre-commit hooks](../../docs/pre-commit.md) (`tpt-lean-fmt` and
`tpt-lean-sorry`), and the [GitHub Action](../../docs/github-action.md) (`ci --sarif`).

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

## Shell completions

```
tpt-lean completions powershell > tpt-lean.ps1
tpt-lean completions bash > /etc/bash_completion.d/tpt-lean
```

The completion scripts are generated from the same command table the help text uses, and a test
checks the two against each other.

## Exit codes

Exit codes come from `tpt-lean-core`'s `ErrorCode::exit_code`. An unknown command is an invalid
argument (exit 2). Commands that are planned but not built fail with `not_implemented` and exit
code 10.

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
