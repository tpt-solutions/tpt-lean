# Changelog

All notable changes to `tpt-lean-core` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

Initial development version `0.1.0`. Nothing has been published yet.

### Added
- Configuration model (`project`, `build`, `index`, `server`, `security`) with loading and
  validation.
- Configuration precedence: defaults, project file, `.tpt-lean/config.toml`, environment,
  then command-line flags.
- Error types with stable string codes and exit-code mapping (`ErrorCode`).
- Project discovery from `lakefile.lean`, `lakefile.toml`, and `tpt-lean.toml`.
- Toolchain detection from `lean-toolchain`.
- Path normalization for Windows, Unix, UNC, and `\\?\` paths.
- SHA-256 file hashing and cache key types.
- Process execution policy types: timeout, output limit, cancellation token, environment
  sanitization.
- JSON envelope and error envelope types with a versioned schema.
- Per-OS global cache directory resolution.
- Logging initialization controlled by `TPT_LEAN_LOG`.

### Known limitations
- Memory limits are part of the process policy but are not enforced by any runner yet.
