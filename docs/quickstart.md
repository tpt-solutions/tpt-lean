# Quickstart

This takes about five minutes, from a new project to a search, a format check and a build. It
assumes tpt-lean is built and on your `PATH` (see [install.md](install.md)), and that `lean` and
`lake` are installed.

## 1. Create a project

```sh
tpt-lean init hello
cd hello
```

`init` writes `lakefile.lean`, `lean-toolchain`, `Main.lean`, `tpt-lean.toml`, `.gitignore` and
`README.md`:

```text
created hello (6 files)
```

`Main.lean` holds a small program:

```lean
def main : IO Unit :=
  IO.println "Hello from hello"
```

## 2. Build it

```sh
tpt-lean build
```

The first build takes a while, because Lake compiles everything. The output ends with a summary:

```text
build succeeded: 1 file(s) built, 0 failed, 14708 ms (cache key 4 ms)
```

Add `--json` to get one event per line instead, as Lake reports the files:

```sh
tpt-lean build --json
```

## 3. Index the project and search it

```sh
tpt-lean index scan
tpt-lean search main
```

```text
scanned 2 files: 2 updated, 0 unchanged, 0 removed
0.640  def       main  (Main.lean:1)
```

The index holds declaration names and docstrings. `search` matches names first, then docstrings.
`tpt-lean search main --namespace Demo` and `--limit 5` narrow the results.

## 4. Check the formatting

Add a file with extra spaces and let the formatter report it:

```sh
printf 'def  x := 1   \n' > Messy.lean
tpt-lean fmt --check Messy.lean
```

```text
would reformat Messy.lean
```

The exit code is 1 when a file would change, so this works in a script. Use `tpt-lean fmt --diff
Messy.lean` to see the change, and `tpt-lean fmt Messy.lean` to write it. Passing a clean project to
`fmt --check` prints nothing and exits 0.

## 5. Ask the environment for its state

```sh
tpt-lean doctor
tpt-lean --json doctor
```

`doctor` lists the Lean and Lake versions it found, whether it is in a project, and the state of the
configuration, index, cache and language server. Where it can, a failure comes with a suggestion.

## 6. Use it from a script or an agent

Every command takes `--json`:

```sh
tpt-lean --json search main --limit 1
```

```json
{"schema_version":1,"tool":"tpt-lean","kind":"search","ok":true,"data":{"count":1,"items":[{"docstring":null,"file":"Main.lean","kind":"def","line":1,"name":"main","package":"project","score":0.64}]}}
```

See [json-schemas.md](json-schemas.md) for the envelope and every kind.

For a Lean position, `context-pack` gives an agent a bundle of the snippet, the goal, the diagnostics
and nearby declarations:

```sh
tpt-lean context-pack Main.lean:1:5 --max-tokens 2000
```

## Next steps

- **Editors.** `tpt-lean lsp` is a language server (hover goals, symbols, definitions, code actions,
  formatting). [editors.md](editors.md) has setup for VS Code, Neovim, Emacs and Helix.
- **A long-running server.** `tpt-lean server start` serves JSON-RPC on a named pipe (Windows) or a
  Unix socket, and `--http 7373` adds HTTP. A running server keeps one Lean session warm for
  `context.pack`. See [server-api.md](server-api.md).
- **AI assistants.** `tpt-lean mcp stdio` runs the MCP server. The project's `.mcp.json` example is in
  [mcp.md](mcp.md).
- **Profiling.** `tpt-lean profile Main.lean` reports where Lean spends its time, per declaration.
- **CI.** `tpt-lean ci --json` builds the project and reports diagnostics; `--sarif` writes a SARIF
  log for code scanning.
- **Every command and flag.** [cli.md](cli.md).
