# Profiling (`tpt-lean-profile`)

The profiler answers "why is this Lean file slow?" by running the installed `lean` on an
instrumented copy of a file and reading Lean's own profiler output.

## What is measured

- **Per-file wall time**: the duration of the `lean` process.
- **Per-declaration time**: header elaboration, asynchronous proof elaboration and kernel checking,
  attributed to each `theorem`, `def`, `instance`, `example`, and so on.
- **Main cost** per declaration: the trace category with the largest self time in that declaration
  (typeclass resolution, unification, whnf reduction, simp/search, kernel checking, elaboration).
- **Tactic totals** for the file, from Lean's `tactic execution of ... took ...` lines.
- **Cumulative categories** from Lean's `cumulative profiling times` block.
- **Suggested actions**, derived only from these numbers.

## How it works

The profiler writes a temporary copy of the file with one extra line after the imports:

```
set_option profiler true set_option profiler.threshold 1 set_option trace.profiler true set_option trace.profiler.threshold 2
```

It then runs `lean <copy>` through the adapter's typed `Command::LeanCheck`, with the project root as
the working directory. The copy is deleted afterwards. Line numbers in the report refer to the
original file: they are found by searching the original source for each declaration.

Observed Lean 4.36.0 output:

- Trace nodes are printed as `[Tag] [seconds] <status> <text>` with children indented two spaces. They
  appear on stdout. Theorem proofs are elaborated under `[Elab.async] elaborating proof of NAME`, and
  kernel checks under `[Kernel] typechecking declarations [NAME._proof_1_1]`, often nested under an
  `addDeclCore` async node.
- The plain profiler summary (`elaboration took 2.7ms`, the cumulative block, and
  `tactic execution of Lean.Parser.Tactic.omega took 2.3ms`) carries no declaration names, so it is
  used only for file-level and per-tactic totals.
- Only the `ms` unit has been observed. `s`, `µs` and `ns` are parsed defensively.

## Limits

- Declarations whose trace node is under 2 ms are not listed. Their time still counts in the wall time.
- Imports and startup are not attributed. A trivial file spent about 1.7 to 1.9 s in startup on this machine (Lean 4.36.0),
  and the report says so.
- Imports resolve as with plain `lean`: the toolchain library and `LEAN_PATH`. The profiler does not
  run through `lake env`, so files importing project modules need `LEAN_PATH` set to the built
  libraries, or they report errors.
- Anonymous declarations (`instance : ...`, `example`) are matched to source lines by order, which
  can be wrong when earlier anonymous declarations were omitted by the threshold.
- A non-zero Lean exit code is not an error. The report records `lean_errors`, and timings may then
  be incomplete.

## Not implemented

Memory tracking, slow-import detection, dependency bottleneck graphs, and
build-step and cache hit/miss reports. Each is listed in the report's `notes` field.

## Reports

`ProfileReport` serializes to JSON with `format_version` 1. Times are milliseconds, rounded to
0.1 µs. `from_json` rejects other versions.

## API

- `profile_file(project_root, file, timeout_secs) -> Result<ProfileReport>`
- `slowest(&report, n) -> Vec<DeclTiming>`
- `render_report(&report, n) -> String`: the text output
- `compare(&before, &after) -> String`: the text diff
- `suggest(&report) -> Vec<String>`
- `to_json`, `from_json`, `read_report(path)`
