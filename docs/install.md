# Install

tpt-lean is not published yet. There are no release binaries and no crates on crates.io, so you
build it from a checkout of the repository.

## What you need

- **Rust** 1.82 or newer, from [rustup](https://rustup.rs). This builds tpt-lean.
- **Lean and Lake**, for the commands that run Lean (`build`, `check`, `goals`, `explain`,
  `context-pack`, `profile`, `ci`, `bisect`, `index sidecar`, `init --mathlib`). Install them with
  [elan](https://github.com/leanprover/elan). tpt-lean finds them on `PATH` or in `~/.elan/bin`.
  Commands that do not run Lean, such as `search`, `index`, `fmt`, `imports`, `init` and `cache`,
  work without them.

The tested Lean versions are 4.35.0 (the previous stable release) and 4.36.0. Other versions run,
but `doctor` and `explain` report them as untested.

Supported platforms are Windows, Linux and macOS. The test suite has been run on Windows only, in
this project's development environment. The CI workflow covers all three, but it has not run on
GitHub yet.

## Build

From the root of the repository:

```sh
cargo build --release -p tpt-lean-cli
```

This produces two equivalent programs in `target/release/`:

- `tpt-lean` (on Windows, `tpt-lean.exe`)
- `tlean`, the same program under a shorter name

Put the directory on your `PATH`. On Windows PowerShell, for the current session:

```powershell
$env:Path += ";$PWD\target\release"
```

Check the install:

```sh
tpt-lean --help
tpt-lean doctor
```

`doctor` reports the Lean and Lake it found, the project if you are in one, and the state of the
setup. Where it can, it also suggests a fix.

## Optional: the Lean sidecar

`tpt-lean index sidecar` needs a small Lean program that reads declaration types. Build it once for
each Lean version you use:

```sh
cd lean/tpt-lean-sidecar
lake -R build
```

For a second Lean version, select it with the elan override and give the build its own directory:

```sh
ELAN_TOOLCHAIN=leanprover/lean4:v4.35.0 lake -R build -K buildDirName=.lake/build-v4.35.0
```

Then run `tpt-lean index sidecar --exe <path to tpt_sidecar>`, or set `TPT_LEAN_SIDECAR` to that
path. The `-R` flag is needed when the build directory changes, because Lake caches its
configuration.

## Optional: the Python and Go helpers

- `scripts/validate-sarif.py` checks `ci --sarif` output against the official SARIF schema. It needs
  Python 3 and `pip install jsonschema`.
- The Go tests in `crates/tpt-lean-ffi/tests/go` load the C library without cgo. Run
  `cargo build -p tpt-lean-ffi`, then `go test ./...` in that directory. They run on Windows.

## Not yet

- No package for crates.io, npm, Homebrew, winget or similar. The WebAssembly package in
  `crates/tpt-lean-wasm/npm` builds and runs locally but is not published.
- No prebuilt binaries.

## Uninstall

Delete the checkout, or remove the directory from `PATH`. tpt-lean stores its global cache in the
user's cache directory: `%LOCALAPPDATA%\tpt-lean\cache` on Windows, `~/Library/Caches/tpt-lean` on
macOS, and `$XDG_CACHE_HOME/tpt-lean` or `~/.cache/tpt-lean` elsewhere. `tpt-lean cache status`
shows where. Project data lives in each project's `.tpt-lean/` directory.
