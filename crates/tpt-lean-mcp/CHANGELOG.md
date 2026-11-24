# Changelog

All notable changes to `tpt-lean-mcp` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

Initial development version `0.1.0`. Nothing has been published yet.

### Added
- Hand-written MCP server over JSON-RPC 2.0 and `serde_json`.
- stdio transport and loopback-only HTTP transport with server-sent events.
- Tools: `tpt_lean_search`, `tpt_lean_context_pack`, `tpt_lean_goals_at`,
  `tpt_lean_check_file`, `tpt_lean_format_text`, `tpt_lean_explain_diagnostic`,
  `tpt_lean_format_file`, `tpt_lean_build_project`, `tpt_lean_imports_suggest`,
  `tpt_lean_index_status`, `tpt_lean_profile_file`.
- Permission classes (`read`, `write`, `build`, `profile`) checked on every call; writes and
  builds are opt-in through configuration.
- Path safety through `WorkspaceRoot`.
- Resources: project info, project diagnostics, index status, file symbols.
- Prompt templates for Lean error explanation, proof-state summary, lemma search, timeout
  reduction, and import suggestions.

### Known limitations
- Build-log and profile resources are not implemented.
- Progress notifications are only partly implemented.
