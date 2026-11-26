-- SPDX-License-Identifier: MIT OR Apache-2.0
import Lean
open Lean Meta

/-!
# tpt-lean sidecar

Two modes, both using only Lean's own APIs (no Mathlib). Lean stays the source of truth: this
program only reads what Lean already computes.

* `tpt_sidecar <Module.Name>`: the declarations of one module (name, kind, pretty type, docstring).
* `tpt_sidecar goals <file.lean> <line> <column>`: the goals before the innermost tactic at a
  position in a source file. The file is elaborated with Lean's frontend, and the goals are read
  from the tactic info trees, the same data Lean's language server uses for `plainGoal`. The line is
  1-based and the column is 1-based, counting characters.

Output of the declarations mode (one JSON object on stdout, schema version 1):

```json
{"schema_version":1,"module":"Init.Prelude","declarations":[
  {"name":"Nat.add","kind":"definition","type":"Nat → Nat → Nat","doc":null}]}
```

Output of the goals mode (schema version 1):

```json
{"schema_version":1,"file":"Main.lean","line":2,"column":3,"goals":["a b : Nat\n⊢ a + b = b + a"]}
```

Exit codes: 0 success, 2 usage error, 3 module could not be imported or found, 4 the file could not
be read or elaborated.

Setting `TPT_SIDECAR_DEBUG=1` prints the elaboration message and the info-tree counts to stderr.
The executable is built with `supportInterpreter` so that Lean's initializers (which register the
syntax of `Init`) can run inside it. Its shared library must be found through `PATH`; tpt-lean adds
the toolchain's `bin` directory.
-/

/-- Version of the JSON schema. Bump on any incompatible change. -/
def schemaVersion : Nat := 1

/-- Stable, lowercase name of a constant's kind. -/
def kindOf : ConstantInfo → String
  | .defnInfo _ => "definition"
  | .thmInfo _ => "theorem"
  | .axiomInfo _ => "axiom"
  | .opaqueInfo _ => "opaque"
  | .quotInfo _ => "quotient"
  | .inductInfo _ => "inductive"
  | .ctorInfo _ => "constructor"
  | .recInfo _ => "recursor"

/-- Pretty-print an expression with the environment's notation. -/
def prettyExpr (env : Environment) (e : Expr) : IO String := do
  let ctx : Core.Context := { fileName := "<sidecar>", fileMap := default }
  let st : Core.State := { env := env }
  let act : CoreM Format := (ppExpr e).run'
  let (fmt, _) ← act.toIO ctx st
  -- A very wide width keeps each type on one line, so the JSON stays one record per line.
  return fmt.pretty (width := 1000000)

/-- Declarations of one module, in the order Lean stored them. -/
def moduleDeclarations (env : Environment) (idx : Nat) : IO (Array Json) := do
  let md := env.header.moduleData[idx]!
  let mut out : Array Json := #[]
  for name in md.constNames, ci in md.constants do
    -- Compiler- and elaborator-generated helpers are not part of the user-facing surface.
    if name.isInternal then
      continue
    let ty ← prettyExpr env ci.type
    let doc ← findDocString? env name
    out := out.push <| Json.mkObj [
      ("name", toJson name.toString),
      ("kind", toJson (kindOf ci)),
      ("type", toJson ty),
      ("doc", match doc with | some d => toJson d | none => Json.null)
    ]
  return out

/-- Turn on initializers, which imports need before they load extensions. The function is unsafe
in Lean, so it is reached through an opaque declaration with an `implemented_by` target. -/
unsafe def enableInitializersUnsafe : IO Unit := enableInitializersExecution

@[implemented_by enableInitializersUnsafe]
opaque initializersEnabled : IO Unit

/-! ## Goals at a position -/

/-- A tactic whose syntax contains the position, with what is needed to print its goals. -/
structure TacticAt where
  ctx : Elab.ContextInfo
  info : Elab.TacticInfo
  /-- Width in bytes of the tactic's syntax; the innermost tactic is the narrowest. -/
  span : Nat

/-- Every tactic in the info trees whose syntax contains `pos`. -/
def tacticsAt (trees : Array Elab.InfoTree) (pos : String.Pos.Raw) : Array TacticAt :=
  trees.foldl (init := #[]) fun acc tree =>
    tree.foldInfo (init := acc) fun ctx info acc =>
      match info with
      | .ofTacticInfo ti =>
        match ti.stx.getPos?, ti.stx.getTailPos? with
        | some start, some stop =>
          if start ≤ pos && pos < stop then
            acc.push { ctx := ctx, info := ti, span := stop.byteIdx - start.byteIdx }
          else acc
        | _, _ => acc
      | _ => acc

/-- The innermost tactic: the one with the narrowest syntax. -/
def innermost? (xs : Array TacticAt) : Option TacticAt :=
  xs.foldl (init := none) fun best t =>
    match best with
    | none => some t
    | some b => if t.span < b.span then some t else some b

/-- The goals before a tactic, printed with Lean's own goal pretty-printer. -/
def goalsBefore (t : TacticAt) : IO (List String) := do
  -- The metavariable context of the moment before the tactic is what the goals refer to.
  let ctx := { t.ctx with mctx := t.info.mctxBefore }
  t.info.goalsBefore.mapM fun g => do
    let fmt ← ctx.runMetaM {} (ppGoal g)
    return fmt.pretty (width := 1000000)

/-- Goals at a 1-based line and 1-based column of `file`. Empty when no tactic is there. -/
def goalsAtFile (file : String) (line col : Nat) : IO (List String) := do
  let input ← IO.FS.readFile file
  let inputCtx := Parser.mkInputContext input file
  let (header, parserState, messages) ← Parser.parseHeader inputCtx
  let (env, messages) ← Elab.processHeader header {} messages inputCtx
  let env := env.setMainModule `TptLeanSidecarGoals
  -- Info trees are recorded only when the info state is enabled.
  -- Elaborate in the calling thread, so that the info trees are in the command state when we read them.
  let opts := Options.empty.setBool `Elab.async false
  let commandState := { Elab.Command.mkState env messages opts with infoState.enabled := true }
  let frontend ← Elab.IO.processCommands inputCtx parserState commandState
  let pos := inputCtx.fileMap.ofPosition { line := line, column := col - 1 }
  let trees := frontend.commandState.infoState.trees.toArray
  if (← IO.getEnv "TPT_SIDECAR_DEBUG").isSome then
    for m in frontend.commandState.messages.toList do
      IO.eprintln s!"debug message: {← m.toString}"
    IO.eprintln s!"debug trees={trees.size} tactics={(tacticsAt trees pos).size} pos={pos.byteIdx}"
  match innermost? (tacticsAt trees pos) with
  | none => return []
  | some t => goalsBefore t

/-- JSON for the goals mode. -/
def goalsReport (file : String) (line col : Nat) (goals : List String) : Json :=
  Json.mkObj [
    ("schema_version", toJson schemaVersion),
    ("file", toJson file),
    ("line", toJson line),
    ("column", toJson col),
    ("goals", Json.arr (goals.map toJson).toArray)
  ]

def main (args : List String) : IO UInt32 := do
  match args with
  | ["goals", file, lineStr, colStr] =>
    match lineStr.toNat?, colStr.toNat? with
    | some line, some col =>
      if line == 0 || col == 0 then
        IO.eprintln "usage: tpt_sidecar goals <file.lean> <line> <column>  (1-based)"
        return 2
      initSearchPath (← findSysroot)
      -- Imports load the parser and elaborator extensions only when initializers are enabled.
      initializersEnabled
      try
        let goals ← goalsAtFile file line col
        IO.println (goalsReport file line col goals).compress
        return 0
      catch e =>
        IO.eprintln s!"cannot get goals in {file}: {e}"
        return 4
    | _, _ =>
      IO.eprintln "usage: tpt_sidecar goals <file.lean> <line> <column>"
      return 2
  | [modStr] =>
    let modName := modStr.toName
    initSearchPath (← findSysroot)
    let env ← try
        importModules #[{ module := modName }] {} 0
      catch e =>
        IO.eprintln s!"cannot import {modStr}: {e}"
        return 3
    match env.header.moduleNames.findIdx? (· == modName) with
    | none =>
      IO.eprintln s!"module not imported: {modStr}"
      return 3
    | some idx =>
      let decls ← moduleDeclarations env idx
      let report := Json.mkObj [
        ("schema_version", toJson schemaVersion),
        ("module", toJson modStr),
        ("declarations", Json.arr decls)
      ]
      IO.println report.compress
      return 0
  | _ =>
    IO.eprintln "usage: tpt_sidecar <Module.Name> | tpt_sidecar goals <file.lean> <line> <column>"
    return 2
