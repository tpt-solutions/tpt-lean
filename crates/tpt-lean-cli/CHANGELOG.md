# Changelog

All notable changes to `tpt-lean-cli` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

Initial development version `0.1.0`. Nothing has been published yet.

### Added
- `tpt-lean` and `tlean` binaries with the same behavior.
- Commands: `init`, `doctor`, `config show`, `fmt`, `index`, `search`, `check`, `goals`,
  `explain` (including `--last-error`), `context-pack`, `build`, `cache`, `profile`, `imports`,
  `docgen`, `ci`, `bisect`, `lsp`, `mcp`, `server`, `clean`.
- Global flags: `--json`, `--ndjson`, `--project`, `--set`, `--verbose`, `--quiet`.
- JSON envelope and NDJSON output for every command.
- Configuration precedence and exit-code conventions shared with the library crates.

### Known limitations
- `profile --flamegraph` and `fmt --verify` are not implemented; they exit with code 10.

### Fixed
- `--help` and `-h` print the help text anywhere on the command line, instead of failing with
  `not_implemented`.
