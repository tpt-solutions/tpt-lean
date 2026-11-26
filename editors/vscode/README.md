# tpt-lean for VS Code

Lean 4 support through `tpt-lean lsp`: diagnostics, goals, hover, definitions, code actions, and
three tpt-lean commands. Requires the `tpt-lean` binary (set `tpt-lean.path` if it is not on PATH).

## Commands

- **Show proof state panel**: goals at the cursor, split into hypotheses and target. Follows the
  cursor unless `tpt-lean.proofStatePanel.followCursor` is off.
- **Search lemmas for the goal at cursor**: candidates from the project index. The chosen name is copied.
- **Explain the error at cursor**: the error and the goals there, in the `tpt-lean` output channel.
- **Restart language server**.

## Settings

- `tpt-lean.path`: the executable (default `tpt-lean`).
- `tpt-lean.enable`: start the server for Lean files (default on).

## Notes

- The Lean 4 extension also runs a Lean server. Running both on one project duplicates diagnostics;
  disable one for the workspace. The extension warns once when it finds the Lean 4 extension.
- Lean output is escaped before it is shown in the panel; the panel has no scripts.

## Building

```
npm install
npm test          # compiles, then runs the rendering tests
npx @vscode/vsce package --no-dependencies   # optional: a .vsix, not published
```

Licensed `MIT OR Apache-2.0`, like the rest of the repository.
