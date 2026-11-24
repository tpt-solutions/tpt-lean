-- SPDX-License-Identifier: MIT OR Apache-2.0
import Lean
open Lean Meta

/-!
# tpt-lean sidecar

Emits structured JSON describing the declarations of one module, using only Lean's own
environment API (no Mathlib). Lean stays the source of truth: this program only reads what
the Lean environment already contains.

Usage: `tpt_sidecar <Module.Name>`

Output (one JSON object on stdout, schema version 1):

```json
{"schema_version":1,"module":"Init.Prelude","declarations":[
  {"name":"Nat.add","kind":"definition","type":"Nat → Nat → Nat","doc":null}]}
```

Exit codes: 0 success, 2 usage error, 3 module could not be imported or found.
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

def main (args : List String) : IO UInt32 := do
  match args with
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
    IO.eprintln "usage: tpt_sidecar <Module.Name>"
    return 2
