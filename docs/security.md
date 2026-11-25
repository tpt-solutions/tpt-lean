# Security model

tpt-lean is a local tool. It drives Lean and Lake, serves editors and agents, and reads and writes
project files. This page describes what it allows by default, what each permission class covers,
and what it does not do. Lean remains the trusted proof checker; tpt-lean is not in the trust
path for correctness.

## Default stance

- Local only. The API server and the MCP HTTP transport bind to loopback. Remote access is off.
- Workspace only. Paths must resolve inside the project root (or a configured workspace root).
- Read-heavy. Search, index status, diagnostics, and formatting in memory are available without
  any grant.
- Explicit write permission before any source file is changed.
- Explicit build and profile permission before any Lean or Lake process runs.
- No arbitrary shell execution. Every process is built from typed options (see Command safety).

## Permission classes

| Class   | What it allows                                                        | Granted by                                                |
|---------|-----------------------------------------------------------------------|-----------------------------------------------------------|
| read    | search, symbol and index queries, diagnostics, in-memory formatting   | always                                                    |
| write   | formatting a file on disk                                             | `security.allow_write = true`                             |
| build   | `lean` and `lake` runs, goals, explanations, context packs            | `security.allow_build = true` (default `true`)            |
| profile | timed `lean` runs for profiling                                       | `security.allow_build` and `security.allow_profile` (both default `true`) |

The MCP server enforces these per tool and returns a tool error naming the missing permission; see
[mcp.md](mcp.md#permissions). The API server applies the same classes per method; see
[server-api.md](server-api.md). The design spec also lists a network class and an "apply import
suggestion" class. Neither exists as a separate permission yet: import suggestions are read-only,
and tpt-lean does not fetch anything over the network (see below).

Settings live in `.tpt-lean/config.toml` under `[security]`. Environment variables
`TPT_LEAN_SECURITY_ALLOW_WRITE`, `TPT_LEAN_SECURITY_ALLOW_BUILD`, and
`TPT_LEAN_SECURITY_ALLOW_PROFILE` override them. Precedence is described in [cli.md](cli.md).

Note: `allow_build` defaults to `true` in the current configuration model, so a project that
should be read-only must set it to `false` explicitly.

## Path safety

Every path argument is checked against the workspace roots before use. Rejected inputs include:

- `../` sequences that leave the root, and absolute paths outside it
- symlinks that point outside the root (the file scan never follows symlinks)
- device files and other non-regular files
- tools that read or write source accept `.lean` files only

The path helpers normalize Windows and Unix separators, case, UNC prefixes, and `\\?\` prefixes
before the check. Fuzz-style tests cover these cases in `tpt-lean-core` and `tpt-lean-tools`.

## Command safety

Callers never supply a command line. A request names a project, a target, a file, and limits, and
tpt-lean builds the `lean` or `lake` invocation itself. A request such as
`{"command": "rm -rf /"}` has no meaning in the API and is not accepted.

The `lsp` command starts `lake serve` or `lean --server` for the project, with fixed arguments.

## Process limits

Every Lean or Lake process started by the runner has:

- a timeout (`security.command_timeout_seconds`, default 120), after which the whole process tree
  is killed
- cancellation, which kills the whole process tree
- an output size limit
- a memory limit where the platform supports it (a sampling watchdog over the process tree; there
  are no job objects on Windows yet, so the limit is approximate there)
- a sanitized environment: only variables on the allowlist are passed through, plus any variables
  the caller sets explicitly

The long-lived LSP client is sanitized, but it has no timeout or memory limit because it is a
server process. This is a known gap, tracked in [todo.md](../todo.md).

## What tpt-lean does not do

- It does not make network requests itself. Lake and Lean may fetch packages when a project is
  built or updated; those requests are made by Lake, not by tpt-lean.
- It does not execute arbitrary code from the index, the cache, or the project. Lean elaborates
  project files when a build or check runs; that is the same trust decision as running `lake build`
  yourself.
- It does not treat the index or the build cache as authoritative. Both are acceleration only, and
  Lean's output is the source of truth.
- It does not publish artifacts or send data anywhere. The cache, index, logs, and profiles stay in
  `.tpt-lean/` inside the project and in the per-user cache directory.

## Reporting

[CONTRIBUTING.md](../CONTRIBUTING.md) accepts issues only. No private channel for security reports
is documented yet; until one is, report security problems to TPT Solutions directly rather than in a
public issue.
