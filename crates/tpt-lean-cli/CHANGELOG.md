# Changelog

All notable changes to `tpt-lean-cli` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

Initial development version `0.1.0`. Not yet published; see [PUBLISHING.md](../../PUBLISHING.md).

### Added
- `tpt-lean` and `tlean` binaries with the same behavior.
- Commands: `init`, `doctor` (with `--fix`), `config show`, `fmt` (with `--check`, `--diff`),
  `index` (including `sidecar`), `search` (with `--from`, `--type`, `--goal-at`), `check`, `goals`
  (with `--sidecar`), `explain` (including `--last-error`), `context-pack`, `build` (with `--watch`,
  `--plan`, `--report`, `--explain-slow`), `cache` (with `export` and `import`), `profile`,
  `imports`, `docgen`, `ci` (with `--sarif`), `bisect`, `upgrade`, `sorry-report`, `watch`, `setup`,
  `completions`, `lsp`, `mcp`, `server` (with `--http`), `clean`.
- `init --template mathlib` as an alias for `--mathlib`.
- `init --mathlib` pins Mathlib to the tag v4.36.0 and sets `lean-toolchain` to match. A full
  Mathlib build was not run.
- `upgrade --to VERSION [--apply]`: plans a `lean-toolchain` change; see `tpt-lean-tools`.
- `watch [--once] [--interval-ms N]`: re-checks each changed `.lean` file with `lean` and prints
  errors, warnings and `sorry` counts. `--once` exits 1 on any error. Polls file stamps; does not
  run Lake.
- `sorry-report [paths] [--deny-sorry]`.
- `setup [--editor NAME]... [--mcp NAME]... [--dry-run]`: writes Helix, VS Code, Neovim and Emacs
  language-server config, and `.mcp.json` and `.cursor/mcp.json` entries. Merges with existing
  files, never overwrites a differing `tpt-lean` entry, shows a diff, and `--dry-run` exits 1 when
  a change is pending.
- `completions bash|zsh|fish|powershell`, generated from one command table that a test checks
  against the help text.
- `doctor --fix`: prints a plan, then rebuilds the symbol index when it is missing or damaged.
- `build --plan [--json]` and `build --report`.
- `cache export DIR` and `cache import DIR`, for CI artifacts.
- `search --type PATTERN`, `search --from FILE`, and `search --goal-at FILE:LINE:COL`.
- `goals FILE:LINE:COL --sidecar`: goals from the Lean sidecar, without a language server.
- `server start --http PORT`: loopback HTTP transport.
- Pre-commit hooks (`.pre-commit-hooks.yaml`): `tpt-lean-fmt` and `tpt-lean-sorry`. See
  `docs/pre-commit.md`.
- GitHub Action: a composite `action.yml` at the repository root. It installs tpt-lean from a
  release, runs `tpt-lean ci --sarif`, and uploads the result to code scanning. See
  `docs/github-action.md`. Not yet run on GitHub.
- VS Code extension in `editors/vscode/` (not published). It starts `tpt-lean lsp`.
- Global flags: `--json`, `--ndjson`, `--project`, `--set`, `--verbose`, `--quiet`.
- JSON envelope and NDJSON output for every command.
- Configuration precedence and exit-code conventions shared with the library crates.
- Release packaging, not yet run on GitHub: `.github/workflows/release.yml`, `scripts/install.sh`
  and `scripts/install.ps1`, manifests under `packaging/` (Homebrew, Scoop, winget), and
  `[package.metadata.binstall]` for `cargo binstall`.

### Changed
- `tpt-lean init` no longer leaves an empty directory behind when the template name is unknown.
- The help text lists the implemented commands; before, it listed built commands as planned.
- An unknown command is an invalid argument (exit 2), not "not implemented" (exit 10).

### Fixed
- `--help` and `-h` print the help text anywhere on the command line, instead of failing with
  `not_implemented`.

### Known limitations
- `profile --flamegraph` and `fmt --verify` are not implemented; they exit with code 10.
- The VS Code extension has not been run in a VS Code host.
