# tpt-lean-core

Shared foundation for [tpt-lean](https://github.com/tpt-solutions/tpt-lean), a Rust toolbox for
Lean 4. This crate holds the types and rules that every other tpt-lean crate depends on:
configuration, error codes, project discovery, toolchain detection, paths, hashing, process
execution policy, the JSON envelope, and logging.

It contains no Lean semantics. Lean remains the trusted proof checker; nothing in this crate
parses, elaborates, or checks Lean code.

## Status

Early development (version `0.1.0`, not yet published). The configuration model, error
codes, discovery, and path handling are implemented and tested. The process policy types are
implemented. The memory limit in the policy is enforced by the sampling watchdog in
`tpt-lean-adapter`'s runner, not by the operating system.

## What it provides

- **Configuration** (`config`): the `tpt-lean.toml` model with five sections: `project`,
  `build`, `index`, `server`, and `security`. `ConfigLoader` and `ConfigSources` implement the
  precedence order: built-in defaults, project file, `.tpt-lean/config.toml`, environment
  variables, then command-line flags. `validate` rejects invalid values and `to_toml` writes
  the effective configuration back out.
- **Errors** (`error`): `Error` and `ErrorCode`. Every error carries a stable string code
  (for example `not_implemented`) and a mapping to a process exit code. The CLI, server, and
  MCP rely on these codes, so new codes are added rather than renamed.
- **Project model** (`project`, `dirs`): `Project` and `discover`, which walk upward from a
  directory looking for `lakefile.lean`, `lakefile.toml`, or `tpt-lean.toml`. `read_toolchain_file`
  reads `lean-toolchain`. `global_cache_dir` picks the per-OS cache location (XDG,
  `~/Library/Caches`, or `%LOCALAPPDATA%`). The project data directory is `.tpt-lean/`.
- **Paths** (`paths`): `lexical_normalize` and `strip_verbatim` make Windows, Unix, UNC, and
  `\\?\` paths comparable without touching the filesystem.
- **Hashing and cache keys** (`hash`): `sha256_hex`, `file_sha256`, and `CacheKey`. SHA-256 is
  used throughout; the project chose it over BLAKE3 to avoid an extra dependency.
- **Process policy** (`policy`): `ProcessPolicy` (timeout, output limit, cancellation),
  `CancelToken`, `Parallelism`, `sanitize_env`, and `default_env_allowlist`. External commands
  are launched with a sanitized environment.
- **JSON schema conventions** (`schema`): `Envelope` and `ErrorEnvelope`, the single JSON object
  every `--json` command prints, and `SCHEMA_VERSION`. Field names are stable and versioned.
- **Logging** (`init_logging`): writes to stderr. Set `TPT_LEAN_LOG` to a filter such as
  `debug` or `tpt_lean_core=trace`. Calling it more than once is safe.

## Usage

This crate is a library. End users normally reach it through the `tpt-lean` CLI. Other
crates in the workspace depend on it directly:

```toml
[dependencies]
tpt-lean-core = "0.1"
```

Its public modules are `config`, `dirs`, `error`, `hash`, `paths`, `policy`, `project`, and
`schema`. The crate root re-exports the most-used items; see the generated API documentation
for the full list.

## Design rules

- **No shell.** Commands are never built from strings. Process policy types exist so that
  other crates can enforce timeouts, output limits, and environment filtering the same way.
- **Stable error codes.** Callers may match on `ErrorCode`; new codes are added, existing codes
  are not renamed.
- **Index and cache data are accelerators.** Hashes and cache keys here only decide whether to
  recompute; they never certify a Lean result.

## Dependencies

| Crate | Purpose |
| --- | --- |
| `serde` | configuration and envelope types |
| `toml` | reading and writing `tpt-lean.toml` |
| `sha2` | SHA-256 hashing |
| `tracing`, `tracing-subscriber` | logging (`fmt` and `env-filter` only) |

All dependencies are licensed under the project's allow list (see `docs/dependencies.md` in
the repository).

## Testing

```
cargo test -p tpt-lean-core
```

The crate has unit tests, including coverage of configuration loading and path handling.

## Related documentation

- [CLI reference](https://github.com/tpt-solutions/tpt-lean/blob/master/docs/cli.md) (global flags, exit codes, configuration precedence)
- [Dependency policy](https://github.com/tpt-solutions/tpt-lean/blob/master/docs/dependencies.md)

## License

Licensed under either of [MIT](https://opensource.org/licenses/MIT) or
[Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0), at your option.
