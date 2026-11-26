# pre-commit hooks

The repository root has `.pre-commit-hooks.yaml`, which defines two hooks for the
[pre-commit](https://pre-commit.com) framework. Both run the `tpt-lean` binary on the staged
`.lean` files:

| Hook id | Runs | Fails when |
|---|---|---|
| `tpt-lean-fmt` | `tpt-lean fmt --check` | a file would be reformatted |
| `tpt-lean-sorry` | `tpt-lean sorry-report --deny-sorry` | a file contains `sorry` or `admit` |

The hooks use `language: system`, so `tpt-lean` must already be on PATH. Install it first
(`docs/install.md`), then add this to the project's `.pre-commit-config.yaml`:

```yaml
repos:
  - repo: https://github.com/tpt-solutions/tpt-lean
    rev: v0.1.0            # any release tag or commit
    hooks:
      - id: tpt-lean-fmt
      - id: tpt-lean-sorry
```

Run `pre-commit run --all-files` once to check the whole project.

Notes:

- `tpt-lean-sorry` is strict by design. Keep it for finished files and skip it on work in
  progress by listing `tpt-lean-sorry` under `stages: [manual]` or by using `SKIP=tpt-lean-sorry`
  for one commit.
- The hooks read files as they are on disk. Stage the whole file, not part of it, or the check
  sees content that is not committed.
- `tpt-lean` locates the project from the current directory, so run `git commit` from inside the
  repository as usual.
