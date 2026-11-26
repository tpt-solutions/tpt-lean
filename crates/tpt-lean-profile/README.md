# tpt-lean-profile

Timing and profiling reports for Lean 4 files, part of
[tpt-lean](https://github.com/tpt-solutions/tpt-lean).

The profiler runs Lean with its own trace output enabled and turns the result into a report:
per-file wall time, per-declaration timings, tactic totals, a ranking of the slowest
declarations, and suggestions. Reports are JSON and can be compared against an earlier run to
see what got slower.

The timings are Lean's own measurements. This crate parses and ranks them; it does not
estimate or invent costs.

## Status

Early development (`0.1.0`, not yet published). Running a profile, parsing Lean's trace and
summary output, analysis, rendering, JSON reports, and comparison are implemented and tested.
The MCP server exposes profiling as `tpt_lean_profile_file`.

## What it provides

- **Running** (`profile_file`, `run`): runs Lean on one file with trace profiling enabled and
  collects its output.
- **Parsing** (`parse_trace`, `parse_summary`, `parse_decl_header`, `parse_duration_ms`):
  reads Lean's profiler lines into per-declaration and per-tactic timings.
- **Analysis** (`analyze`, `Analysis`, `slowest`, `subtree`, `suggest`): totals, rankings,
  the call tree under a declaration, and suggestions. `cost_label` gives a short label for a
  timing.
- **Reports** (`ProfileReport`, `to_json`, `from_json`, `read_report`, `render_report`):
  reports carry `FORMAT_VERSION` and a `notes` list of what was not measured.
- **Comparison** (`compare`): differences between two reports.
- **Rounding and formatting** (`round_ms`, `format_ms`): consistent presentation.

## Usage

```
tpt-lean profile FILE [--slowest N] [--json]
tpt-lean profile --compare A.json B.json
```

Imports resolve the same way plain `lean` does, so a file that imports project modules needs
`LEAN_PATH` set.

```toml
[dependencies]
tpt-lean-profile = "0.1"
```

## Limits

Not implemented, and listed in each report's `notes`:

- memory tracking,
- flamegraph output (`tpt-lean profile --flamegraph` returns `not_implemented`),
- slow-import detection,
- dependency bottleneck graphs.

Build-step timing and cache hit/miss reports are produced by the CLI (`tpt-lean profile --build`),
not by this crate.

The MCP server exposes this crate as the `tpt_lean_profile_file` tool, gated by `security.allow_build`.

## Dependencies

| Crate | Purpose |
| --- | --- |
| `tpt-lean-core` | errors, paths, policy |
| `tpt-lean-adapter` | running `lean` with trace options |
| `serde`, `serde_json` | report format |

## Testing

```
cargo test -p tpt-lean-profile
```

Tests use fixture files in `tests/fixtures/` (a sample profiler output, a sample trace, and a
sample Lean file). `tests/lean_integration.rs` runs a real Lean installation.

## License

Licensed under either of [MIT](https://opensource.org/licenses/MIT) or
[Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0), at your option.
