-- SPDX-License-Identifier: MIT OR Apache-2.0
import Lake
open Lake DSL

package tpt_lean_sidecar where
  version := v!"0.1.0"

@[default_target]
lean_exe tpt_sidecar where
  root := `Main
