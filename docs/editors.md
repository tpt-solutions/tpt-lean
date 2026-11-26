# Editor setup

`tpt-lean lsp` runs a language server that editors can use. It answers some requests itself and
forwards the rest to Lean's own server, so the editor sees one server.

```sh
tpt-lean lsp --project /path/to/project
```

The server talks LSP over stdin and stdout. It needs a Lean project (a directory with
`lakefile.lean` or `lakefile.toml`) and a working `lean` / `lake` on the `PATH`, or the toolchain
managed by `elan`.

What the server answers itself: `documentSymbol` (from the syntax layer, so it works before Lean
finishes), `workspace/symbol` (from `.tpt-lean/index.sqlite`, when that exists), and `hover` (the
goal at the position). Everything else is forwarded to Lean. Diagnostics come from Lean.

Not available yet: go-to-definition and code actions (import suggestions, lemma search,
explain-error). Requests for these are forwarded to Lean, which may or may not answer them; see
[lsp.md](lsp.md).

The settings below have not been tried in each editor. Check them against your editor's version.

## VS Code

VS Code has no built-in way to run an arbitrary language server. Use a generic LSP client extension
that accepts a custom server command, or a small extension of your own built on
`vscode-languageclient`. Either way the server command is:

```
tpt-lean lsp --project ${workspaceFolder}
```

with the file type `lean` and the root markers `lakefile.lean` and `lakefile.toml`.

The repository has a VS Code extension in `editors/vscode/`. It starts `tpt-lean lsp` for Lean files
and adds the commands `tpt-lean: Show proof state panel`, `Search lemmas for the goal at cursor` (the
chosen name is copied to the clipboard), `Explain the error at cursor` (output channel) and `Restart
language server`. It is built from source and is not published: `npm install && npm run compile` in
`editors/vscode`, then package it yourself if you want a `.vsix`. Settings: `tpt-lean.path` (the
executable) and `tpt-lean.enable`.

Lean's own VS Code extension already runs Lean's server. Running both on the same project will
duplicate diagnostics, so disable one of them for the workspace.

## Neovim (nvim-lspconfig style)

```lua
vim.lsp.config("tpt_lean", {
  cmd = { "tpt-lean", "lsp", "--project", "." },
  filetypes = { "lean" },
  root_markers = { "lakefile.lean", "lakefile.toml" },
})
vim.lsp.enable("tpt_lean")
```

On Neovim releases that predate `vim.lsp.config`, register the same command through
`require("lspconfig.configs")` with `default_config` set to the same values.

## Emacs (eglot)

```elisp
(add-to-list 'eglot-server-programs
             '(lean4-mode . ("tpt-lean" "lsp" "--project" ".")))
```

## Helix

`languages.toml` in your Helix config directory:

```toml
[language-server.tpt-lean]
command = "tpt-lean"
args = ["lsp", "--project", "."]

[[language]]
name = "lean"
language-servers = ["tpt-lean"]
```

Use `--project` with an absolute path if the editor does not start the server in the project
directory.

## Troubleshooting

- Run `tpt-lean doctor` from the project directory. It checks Lean, Lake, the toolchain, and the
  index.
- If the server exits at startup, run the command by hand in a terminal and read stderr. Startup
  failures are also reported to the editor with `window/logMessage`.
- The server restarts Lean once after a crash, and cancelled requests are not retried. See
  [lsp.md](lsp.md) for details.
