# Changelog

All notable changes to `tpt-lean-mcp` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

Initial development version `0.1.0`. Not yet published; see [PUBLISHING.md](../../PUBLISHING.md).

### Added
- Hand-written MCP server over JSON-RPC 2.0 and `serde_json`. No MCP SDK.
- stdio transport, and loopback-only HTTP transport with server-sent events.
- Protocol revisions 2026-07-28, 2025-11-25, 2025-06-18, 2025-03-26 and 2024-11-05. `initialize`
  negotiates; `server/discover` lists them; an unsupported version gets error -32022 with the
  supported list.
- Tools: `tpt_lean_search`, `tpt_lean_context_pack`, `tpt_lean_goals_at`,
  `tpt_lean_check_file`, `tpt_lean_format_text`, `tpt_lean_explain_diagnostic`,
  `tpt_lean_format_file`, `tpt_lean_build_project`, `tpt_lean_imports_suggest`,
  `tpt_lean_index_status`, `tpt_lean_profile_file`.
- Progress: a `tools/call` with a progress token gets progress messages before its reply.
- Cancellation over stdio: requests run on a worker while input is still read, so a cancellation
  stops a running tool (a Lean check is killed). Tested with a real slow check.
- Permission classes (`read`, `write`, `build`, `profile`) checked on every call; writes and
  builds are opt-in through configuration.
- Path safety through `WorkspaceRoot`.
- Resources: project info and diagnostics, index status, stored profiles (`tpt-lean://profiles`,
  `tpt-lean://profile/{id}`), build log (`tpt-lean://build/log`), and file symbols, syntax tree
  (`/ast`) and goals (`/goals/{line}/{column}`).
- Prompt templates for Lean error explanation, proof-state summary, lemma search, timeout
  reduction, and import suggestions.

### Fixed
- Server-sent event responses over HTTP use CRLF line endings, as the protocol requires.

### Known limitations
- Progress notifications are sent only for `tools/call` with a progress token.
- Profile memory tracking and flamegraphs are not available through the profile tool.
