# CLI reference

Binaries: `tpt-lean` and the short alias `tlean`.

## Global flags

| Flag | Meaning |
| --- | --- |
| `--json` | print one JSON envelope (`schema_version`, `tool`, `kind`, `ok`, `data`) |
| `--ndjson` | print one JSON record per line for list results |
| `--project <dir>` | project root; default: nearest `tpt-lean.toml` or `lakefile.*` above the working directory |
| `--set section.key=value` | configuration override (highest precedence), repeatable |
| `-v`, `--verbose` | debug logging to stderr (or set `TPT_LEAN_LOG`) |
| `-q`, `--quiet` | errors only |

## Configuration precedence

defaults < `tpt-lean.toml` < `.tpt-lean/config.toml` < environment (`TPT_LEAN_<SECTION>_<KEY>`) <
`--set`. See `tpt_lean_core::config` for the list of recognised environment variables.

## JSON errors

On failure with `--json`, stdout contains:

```json
{"schema_version":1,"tool":"tpt-lean","kind":"error","ok":false,"error":{"code":"timeout","message":"...","path":null}}
```

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | success |
| 1 | a check failed (`fmt --check` would change files, or Lean reported errors) |
| 2 | invalid argument or unknown flag |
| 3 | Lean/Lake or project not found |
| 4 | configuration invalid |
| 5 | path outside the workspace |
| 6 | index error |
| 7 | syntax invalid or uncertain (formatter refused) |
| 8 | timeout or output limit exceeded |
| 9 | I/O error |
| 10 | not implemented yet |
| 70 | internal error |

## Commands

- `init <dir> [--mathlib]`
- `doctor`
- `config show`
- `fmt [--check] [--diff] [--verify] [paths...]` (`--verify` needs Lean and is not implemented yet)
- `search <query> [--namespace NS] [--near TERM]... [--limit N]`
- `index scan | refresh | status | clear`
- `build [target] [--timeout SECONDS]`
- `check [file] [--timeout SECONDS]`
- `cache status`
