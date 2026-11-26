# GitHub Action

The repository root holds a composite action, `action.yml`. It builds a Lean project with
`tpt-lean ci --sarif`, uploads the diagnostics to code scanning, and fails the job when the
build fails.

```yaml
name: Lean
on: [push, pull_request]

permissions:
  contents: read
  security-events: write   # for the SARIF upload

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: tpt-solutions/tpt-lean@v0.1.0
        with:
          deny-sorry: "true"
```

Replace `v0.1.0` with the release you want. The action installs that release through
`scripts/install.sh` (Linux and macOS) or `scripts/install.ps1` (Windows), and the installer
checks the archive against `SHA256SUMS` before it extracts anything.

## Inputs

| Input | Default | Meaning |
|---|---|---|
| `version` | `latest` | Release tag of tpt-lean to install. |
| `project` | `.` | Lean project directory, relative to the workspace. |
| `sarif-file` | `tpt-lean.sarif` | Where the SARIF log is written. |
| `upload-sarif` | `true` | Upload the SARIF log to code scanning. A failed upload does not fail the job. |
| `deny-sorry` | `false` | Fail the job when `sorry` or `admit` is present (`sorry-report --deny-sorry`). |
| `install-lean` | `true` | Install elan on Linux and macOS. Lake then fetches the toolchain in `lean-toolchain`. |

Output: `exit-code`, the status of `tpt-lean ci`.

## Notes

- **Windows runners** do not get Lean from this action. Install Lean first, for example with the
  elan installer, and make sure `lean` and `lake` are on PATH.
- **Code scanning** needs `security-events: write`. On a private repository without code
  scanning enabled the upload step is skipped with a warning, and the job result is unchanged.
- **Lean install** uses the official `elan-init.sh` from the `leanprover/elan` repository, with
  the toolchain left to Lake. Pin the script to a commit if your policy requires it.
- The action is not yet exercised on GitHub. Run it on a test repository before you depend on it.
