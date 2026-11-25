# MCP server

`tpt-lean-mcp` implements the Model Context Protocol (revision `2024-11-05`) over JSON-RPC 2.0.
It exposes tools, read-only resources, and prompt templates for a single project.

## Running it

```sh
tpt-lean mcp stdio --project /path/to/project
```

Only protocol messages are written to stdout. The HTTP transport (loopback only) is
implemented in the crate as `tpt_lean_mcp::serve_http`; the CLI does not start it yet.

### Client setup

Claude Code (project scope, `.mcp.json` at the project root):

```json
{
  "mcpServers": {
    "tpt-lean": {
      "command": "tpt-lean",
      "args": ["mcp", "stdio", "--project", "."]
    }
  }
}
```

Other MCP clients take the same command and arguments in their stdio server configuration.

Claude Desktop (`claude_desktop_config.json`, under the `mcpServers` key). Desktop starts the server
from an unknown working directory, so give the project as an absolute path:

```json
{
  "mcpServers": {
    "tpt-lean": {
      "command": "tpt-lean",
      "args": ["mcp", "stdio", "--project", "C:\\path\\to\\your\\project"]
    }
  }
}
```

Use forward slashes or doubled backslashes in JSON strings. `tpt-lean` must be on the `PATH` that
Desktop sees; if it is not, give the full path to the binary as `command`. This configuration has
not been tested in Claude Desktop yet.

## Permissions

The server is read-only by default. Each tool declares a permission class, and a call whose
class the project config does not grant returns a tool error (`isError: true`) that names the
missing permission. Denied calls do not touch the filesystem or start a process.

| Class   | Granted by                              | Tools |
|---------|-----------------------------------------|-------|
| read    | always                                  | `tpt_lean_search`, `tpt_lean_format_text`, `tpt_lean_index_status`, `tpt_lean_imports_suggest` |
| write   | `security.allow_write = true`           | `tpt_lean_format_file` |
| build   | `security.allow_build = true` (default) | `tpt_lean_check_file`, `tpt_lean_build_project`, `tpt_lean_goals_at`, `tpt_lean_explain_diagnostic`, `tpt_lean_context_pack` |
| profile | `security.allow_build` and `security.allow_profile` (both default to true) | `tpt_lean_profile_file` |

Settings live in `.tpt-lean/config.toml`:

```toml
[security]
allow_write = true
allow_build = true
allow_profile = true
command_timeout_seconds = 120
```

`TPT_LEAN_SECURITY_ALLOW_WRITE`, `TPT_LEAN_SECURITY_ALLOW_BUILD`, and `TPT_LEAN_SECURITY_ALLOW_PROFILE` override them.
Every path argument must resolve inside the project root, and tools that read or write
source only accept `.lean` files.

## Tools

Arguments and results are JSON. Positions are 1-based; columns count characters. Every tool
result is a text block holding the JSON, also returned as `structuredContent`. Each tool
advertises `inputSchema` and `outputSchema` in `tools/list`.

| Tool | Arguments | Result |
|------|-----------|--------|
| `tpt_lean_search` | `query`, `limit` (1-200), `namespace` | `results`: name, kind, package, source, line, score, docstring |
| `tpt_lean_format_text` | `text` | `formatted`, `changed`; never writes |
| `tpt_lean_index_status` | none | schema version, file/symbol/import counts, packages, integrity |
| `tpt_lean_check_file` | `file`, `timeout_seconds` | `success`, `timed_out`, `diagnostics` |
| `tpt_lean_format_file` | `file`, `dry_run` | `changed`, `written`; refuses uncertain input |
| `tpt_lean_goals_at` | `file`, `line`, `column`, `timeout_seconds` | `goals` from the Lean language server |
| `tpt_lean_explain_diagnostic` | `file`, `line`, `column`, `timeout_seconds` | diagnostics covering the position, goal, hypotheses, target |
| `tpt_lean_context_pack` | `file`, `line`, `column`, `max_tokens` (default 6000), `timeout_seconds` | goal, hypotheses, diagnostics, `relevant_symbols`, `source_excerpt`, `estimated_tokens`, `truncated` |
| `tpt_lean_build_project` | `target`, `timeout_seconds` | `success`, `exit_code`, `diagnostics`, `output_tail` (runs `lake build`) |
| `tpt_lean_profile_file` | `file` | the profile report (wall time, per-declaration timings, tactic totals, suggestions); runs Lean |
| `tpt_lean_imports_suggest` | `query`, `limit`, `file` | `suggestions`: module, import statement, matching symbols; modules `file` already imports are left out |

Notes:

- Index tools need an index: run `tpt-lean index scan` first.
- Lean-backed tools start `lake serve` (or `lean --server`) and wait up to the timeout.
  Diagnostics are those published by the time the goal request is answered.
- `tpt_lean_context_pack` ranks index hits for the goal's target first, then declarations
  above the position in the same file. The index stores no types, so symbols carry no `type`.
- Module names for suggestions come from file paths relative to the package root. Lakefile
  source roots are not read.

## Resources

| URI | Content |
|-----|---------|
| `tpt-lean://project/info` | name, root, Lean version, toolchain paths, security settings |
| `tpt-lean://project/diagnostics` | diagnostics cached from the latest check and build runs (empty until one runs) |
| `tpt-lean://index/status` | same as `tpt_lean_index_status` |
| `tpt-lean://file/{path}/symbols` | top-level declarations of one `.lean` file; `{path}` is percent-encoded and workspace-relative |
| `tpt-lean://profiles` | ids of profile reports stored in `.tpt-lean/profiles` by the server's `profile.run` method, newest first |
| `tpt-lean://profile/{id}` | one stored profile report; ids are validated, so no other file can be read |

The MCP `tpt_lean_profile_file` tool does not store its report, so reports appear here only when
they were produced through the API server's `profile.run`. Build logs are not stored by any transport yet.

`resources/list` and `resources/templates/list` describe them; `resources/read` returns JSON.

## Prompts

`prompts/list` and `prompts/get` provide five templates: `explain_lean_error`,
`find_lemma_for_goal`, `reduce_proof_timeout`, `suggest_imports`, and `summarize_proof_state`.
Each fills `{{argument}}` placeholders and points the model at the tools to use.

## Protocol behaviour

- `initialize`, `ping`, `tools/*`, `resources/*`, `prompts/*`.
- An unknown tool name is a JSON-RPC error (`-32602`). Failed tool calls are tool errors.
- Batch requests are rejected.
- `notifications/cancelled` is honoured for requests still pending on the HTTP transport:
  a cancelled request's reply is dropped. `tpt_lean_check_file` and `tpt_lean_build_project`
  pass their cancellation token to the adapter's process runner, which is meant to stop the
  process (not yet verified by a test). Language-server calls cannot be interrupted and run to
  their timeout. Over stdio the loop is synchronous, so a cancel can only refer to a request already
  answered and has no effect.
- Progress notifications are not sent.

## HTTP transport

`serve_http` listens on a loopback address only and fails before binding otherwise. Protocol:

1. `GET /sse` opens an event stream. The first event, `endpoint`, gives `/messages?sessionId=...`.
2. `POST` one JSON-RPC message to that URL. The response is `202 Accepted`; the reply arrives
   as a `message` event on the stream.

Requests are refused (403) when `Host` is not `localhost`, `127.0.0.1`, or `[::1]`, or when an
`Origin` header names another site. A POST must be `application/json` (415 otherwise), which
browsers cannot send cross-origin without a preflight that this server never answers.
