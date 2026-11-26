# tpt-lean-adapter

The controlled bridge between [tpt-lean](https://github.com/tpt-solutions/tpt-lean) and the
official Lean tools, `lean` and `lake`.

Every invocation is built from typed options. Commands are never assembled from strings and
there is no shell. Output is parsed into structured diagnostics and goal values. Lean remains
the authority for everything it reports: this crate forwards Lean's answers and never
substitutes its own.

## Status

Early development (`0.1.0`, not yet published).

| Level | Scope | State |
| --- | --- | --- |
| 1 | CLI integration: `lean` and `lake` version queries, `lake build` with live output, `lean <file>`, diagnostic parsing | implemented |
| 2 | LSP client to `lake serve` or `lean --server`: document sync, diagnostics, plain goals, restart, cancellation, file progress | implemented |
| 3 | Lean sidecar (`lean/tpt-lean-sidecar`): declarations (name, kind, type, docstring) as versioned JSON, and goals before the innermost tactic at a position (`goals FILE LINE COL`) | implemented |

The test suite passes on Lean 4.35.0 and 4.36.0 (run locally). No other Lean version has been
tested.

## What it provides

- **Toolchain discovery** (`discover_toolchain`, `Toolchain`): finds `lean` and `lake` on `PATH`,
  then in `ELAN_HOME/bin` (default `~/.elan/bin`).
- **Typed commands** (`Command`): one variant per supported invocation, each mapped to a fixed
  argument vector. Examples include `lake env printenv` and `lean --version`. Build targets
  pass `validate_target` first: letters, digits, `_`, `.`, and `-` only, with no path
  separators.
- **Process runner** (`runner`, `run`, `run_streaming`, `execute`, `ProcessOutput`,
  `CommandResult`): runs a command with a timeout, an output size limit, cancellation, and a
  sanitized environment (`lean_policy`, `policy_with_timeout`). `run_streaming` passes each output
  line to the caller as it is read; `tpt-lean build` uses it for live progress.
- **Diagnostics** (`diagnostics`, `Diagnostic`, `Severity`): `parse_lean_output` turns Lean's
  messages into severity, file, line, column, and message. Both Lake 5's
  `error: FILE:LINE:COL: message` and the older order are read.
- **Last-run diagnostics** (`last_diagnostics`): `save_last_diagnostics` and
  `load_last_diagnostics` keep the most recent diagnostics on disk so `tpt-lean explain
  --last-error` can read them.
- **LSP client** (`lsp`, `session`, `LspClient`, `LspSession`): speaks JSON-RPC over stdio to
  Lean's server. `server_command` selects `lake serve` or `lean --server`. Helpers include
  `open_document`, `path_to_uri`, `plain_goals`, `plain_term_goal`, and `goal_terms` for the
  `$/lean/plainGoal` and `$/lean/plainTermGoal` requests.
- **Lean sidecar** (`sidecar`): runs `tpt_sidecar` for one module. It returns declarations as
  versioned JSON, and `goals` returns the goals before the innermost tactic at a position, read
  from Lean's info trees. The sidecar is built per toolchain version.
- **Lemma suggestions** (`lemmas`, `suggest_lemmas`): candidate lemmas for a goal, drawn from
  the index rather than from Lean.
- **Version parsing** (`parse_version`): reads the versions printed by `lean` and `lake`.

## Usage

```toml
[dependencies]
tpt-lean-adapter = "0.1"
```

The CLI uses this crate for `tpt-lean check`, `goals`, `explain`, `context-pack`, `build`, and
`watch`.

## Safety properties

- No shell: arguments are passed as vectors, and there is a test asserting this.
- Timeouts, output limits, and cancellation apply to every run.
- The environment is filtered through an allow list before any child process starts.
- Paths are validated by the calling crate against workspace roots before they reach a command.

## Limits

- The memory limit is enforced by sampling, not by the OS: the runner checks the summed
  resident memory of the process tree every 100 ms and kills the tree when it is exceeded, so
  a process can briefly overshoot. Job objects (Windows) and rlimits (POSIX) are not used, which
  keeps the crate free of `unsafe`. Killing uses `taskkill /T` on Windows and a `pgrep` walk on
  POSIX; if the runner itself is killed, its children are not.
- Only Lean 4.35.0 and 4.36.0 are tested. Other versions may work, but they are not verified.
- The sidecar works on one module at a time. Its modules must be built first (`lake build`).
- Goals from the sidecar come from Lean's info trees. The language server's goals are a
  separate path (`$/lean/plainGoal`).

## Dependencies

| Crate | Purpose |
| --- | --- |
| `tpt-lean-core` | policy types, errors, paths |
| `tpt-lean-index` | symbol data for lemma suggestions |
| `tpt-lean-search` | ranking for lemma suggestions |
| `tpt-lean-syntax` | declarations and imports |
| `serde`, `serde_json` | LSP messages and stored diagnostics |

## Testing

```
cargo test -p tpt-lean-adapter
```

The crate has unit tests. `tests/lean_integration.rs` exercises a real `lean` and `lake` install.
`tests/sidecar.rs`, `tests/sidecar_goals.rs` and `tests/sidecar_index.rs` exercise the sidecar.

## Related documentation

- [Build orchestration](https://github.com/tpt-solutions/tpt-lean/blob/master/docs/build.md)
- [LSP proxy](https://github.com/tpt-solutions/tpt-lean/blob/master/docs/lsp.md)

## License

Licensed under either of [MIT](https://opensource.org/licenses/MIT) or
[Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0), at your option.
