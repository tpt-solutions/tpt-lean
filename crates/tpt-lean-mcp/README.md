# tpt-lean-mcp

A [Model Context Protocol](https://modelcontextprotocol.io) server for
[tpt-lean](https://github.com/tpt-solutions/tpt-lean). It lets AI assistants and other MCP
clients search a Lean project, read goals and diagnostics, format files, and build, using the
same typed, permission-checked operations as the CLI.

The protocol layer is hand-written on JSON-RPC 2.0 and `serde_json`. No MCP SDK is used.

## Status

Early development (`0.1.0`, not yet published). The stdio and loopback HTTP transports, the
tools, resources, prompts, progress, cancellation, the permission model, and path safety are
implemented and tested.

## Transports

- **stdio** (`serve_stdio`, `tpt-lean mcp stdio`): newline-delimited JSON-RPC on standard input
  and output. This is the primary transport. Requests run on a worker while input is still read,
  so a cancellation can stop a running tool.
- **HTTP** (`serve_http`, `tpt-lean mcp http`): bound to loopback only, using server-sent events
  for responses. Responses use CRLF line endings.

Protocol revisions: `2026-07-28`, `2025-11-25`, `2025-06-18`, `2025-03-26` and `2024-11-05`
(`PROTOCOL_VERSION` is the preferred one). `initialize` negotiates the revision, and
`server/discover` lists the supported versions. A request that names an unsupported version gets
error `-32022` with the supported list.

## Tools

| Tool | Permission | Purpose |
| --- | --- | --- |
| `tpt_lean_search` | read | search the local index |
| `tpt_lean_format_text` | read | format a text buffer without writing |
| `tpt_lean_index_status` | read | index health and counts |
| `tpt_lean_imports_suggest` | read | suggest imports for a name |
| `tpt_lean_check_file` | build | check a file with Lean |
| `tpt_lean_goals_at` | build | proof goals at a position (runs Lean) |
| `tpt_lean_explain_diagnostic` | build | explain a diagnostic (runs Lean) |
| `tpt_lean_context_pack` | build | assemble context around a position (runs Lean) |
| `tpt_lean_format_file` | write | format a file on disk |
| `tpt_lean_build_project` | build | run `lake build` |
| `tpt_lean_profile_file` | profile | profile a file with timing (runs Lean; gated by `allow_build`) |

A `tools/call` that carries a progress token gets progress messages before its reply.

## Permissions

Each tool declares a permission class: `read`, `write`, `build`, or `profile`. Any tool that runs
Lean is in the `build` class, and so is `profile`, which is gated by the same setting. The server
refuses a call whose class the project configuration does not grant. Reads are allowed by default.
Writes and builds are opt-in through the `[security]` section of `tpt-lean.toml` (`allow_write`,
`allow_build`).

## Path safety

Every path argument is resolved through `WorkspaceRoot`. A path that leaves the project root,
including through symbolic links, device files, or UNC and `\\?\` forms, is rejected before any
file is read.

## Resources

Read-only resources, addressed by `tpt-lean://` URIs:

- `tpt-lean://project/info`: project configuration and toolchain summary.
- `tpt-lean://project/diagnostics`: the most recent diagnostics.
- `tpt-lean://index/status`: index health.
- `tpt-lean://profiles`: stored profile reports.
- `tpt-lean://profile/{id}`: one stored profile report, by the id listed at `tpt-lean://profiles`.
- `tpt-lean://build/log`: the most recent build log.
- `tpt-lean://file/{path}/symbols`: symbols declared in one file.
- `tpt-lean://file/{path}/ast`: the syntax tree of one file.
- `tpt-lean://file/{path}/goals/{line}/{column}`: the goals at a position.

## Prompts

Prompt templates for common Lean tasks: `explain_lean_error`, `summarize_proof_state`,
`find_lemma_for_goal`, `reduce_proof_timeout`, and `suggest_imports`.

## Usage

```
tpt-lean mcp stdio
tpt-lean mcp http
```

A client configuration typically launches `tpt-lean mcp stdio` with the project directory as
its working directory, or passes `--project <dir>`. `tpt-lean setup --mcp NAME` writes the entry
for supported clients.

```toml
[dependencies]
tpt-lean-mcp = "0.1"
```

## Limits

- Progress notifications are sent only for `tools/call` with a progress token.
- Profile memory tracking and flamegraphs are not available through the profile tool.

## Dependencies

Workspace crates `tpt-lean-core`, `tpt-lean-index`, `tpt-lean-search`, `tpt-lean-fmt`,
`tpt-lean-adapter`, `tpt-lean-syntax`, and `tpt-lean-profile`, plus `serde_json`.

## Testing

```
cargo test -p tpt-lean-mcp
```

Tests cover the tools, permission classes, resource URI handling (including traversal attempts),
and refusal of non-loopback HTTP binds.

## Related documentation

- [MCP guide](https://github.com/tpt-solutions/tpt-lean/blob/master/docs/mcp.md)

## License

Licensed under either of [MIT](https://opensource.org/licenses/MIT) or
[Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0), at your option.
