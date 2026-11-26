-- SPDX-License-Identifier: MIT OR Apache-2.0
import Lake
open Lake DSL

package tpt_lean_sidecar where
  version := v!"0.1.0"
  -- One build directory per Lean toolchain: `lake build -K buildDirName=.lake/build-v4.35.0`
  -- under `ELAN_TOOLCHAIN=leanprover/lean4:v4.35.0` keeps a previous-stable binary next to the
  -- default one. The default is `.lake/build`.
  buildDir := (get_config? buildDirName |>.getD ".lake/build" : String)

@[default_target]
lean_exe tpt_sidecar where
  root := `Main
