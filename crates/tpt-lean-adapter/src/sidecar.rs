// SPDX-License-Identifier: MIT OR Apache-2.0
//! Client for the Lean sidecar (`lean/tpt-lean-sidecar`), level 3 of the adapter.
//!
//! The sidecar is a small Lean program that imports one module through Lean's own environment
//! and prints its declarations (name, kind, pretty-printed type, docstring) as one JSON object.
//! This module runs it under the shared process policy and parses the result. The JSON schema
//! is versioned; an unknown version is rejected rather than guessed at.

use crate::runner::{run, ProcessOutput};
use crate::Toolchain;
use serde::Deserialize;
use std::path::Path;
use tpt_lean_core::policy::{CancelToken, ProcessPolicy};
use tpt_lean_core::{Error, ErrorCode, Result};
use tpt_lean_index::{DeclRecord, Index};

/// The sidecar JSON schema version this client understands.
pub const SCHEMA_VERSION: u64 = 1;

/// Exit code the sidecar uses when the module cannot be imported.
const EXIT_MODULE_NOT_IMPORTED: i32 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SidecarReport {
    pub schema_version: u64,
    pub module: String,
    pub declarations: Vec<SidecarDecl>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SidecarDecl {
    pub name: String,
    /// One of: definition, theorem, axiom, opaque, quotient, inductive, constructor, recursor.
    pub kind: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub doc: Option<String>,
}

/// Parse and validate the sidecar's JSON output.
pub fn parse_report(text: &str) -> Result<SidecarReport> {
    let report: SidecarReport = serde_json::from_str(text.trim()).map_err(|e| {
        Error::new(
            ErrorCode::Internal,
            format!("sidecar output is not valid JSON: {e}"),
        )
    })?;
    if report.schema_version != SCHEMA_VERSION {
        return Err(Error::new(
            ErrorCode::Internal,
            format!(
                "sidecar schema version {} is not supported (expected {SCHEMA_VERSION})",
                report.schema_version
            ),
        ));
    }
    Ok(report)
}

/// A Lean module name such as `Init.Prelude` or `My_Lib.Sub-Mod`: dot-separated identifiers.
/// Anything else (spaces, slashes, quotes, leading dots) is refused before it reaches a process.
pub fn validate_module_name(name: &str) -> Result<&str> {
    let ok = !name.is_empty()
        && name.len() <= 300
        && !name.starts_with('.')
        && !name.ends_with('.')
        && name.split('.').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, '_' | '\'' | '-'))
        });
    if ok {
        Ok(name)
    } else {
        Err(Error::invalid(format!("invalid Lean module name: {name:?}")))
    }
}

/// Run the sidecar executable for `module` and return its declarations.
///
/// `exe` is the built `tpt_sidecar` binary. `sysroot` is the Lean toolchain directory; it is
/// passed as `LEAN_SYSROOT` so the sidecar finds Lean's own library without needing `lean` on
/// PATH. `cwd` is the Lake project root when the module comes from a project, so that the
/// search path set by `lake env` (`LEAN_PATH`) applies.
pub fn run_sidecar(
    exe: &Path,
    sysroot: &Path,
    module: &str,
    cwd: Option<&Path>,
    policy: &ProcessPolicy,
    cancel: &CancelToken,
) -> Result<SidecarReport> {
    let module = validate_module_name(module)?;
    let mut policy = policy.clone();
    policy
        .extra_env
        .push(("LEAN_SYSROOT".to_string(), sysroot.display().to_string()));
    let out: ProcessOutput = run(exe, &[module.to_string()], cwd, &policy, cancel)?;
    if out.timed_out || out.cancelled || out.memory_exceeded || out.output_truncated {
        return Err(Error::new(
            ErrorCode::Timeout,
            format!(
                "sidecar did not finish for {module} (timed_out={}, cancelled={}, memory_exceeded={}, output_truncated={})",
                out.timed_out, out.cancelled, out.memory_exceeded, out.output_truncated
            ),
        ));
    }
    match out.exit_code {
        Some(0) => parse_report(&out.stdout),
        Some(EXIT_MODULE_NOT_IMPORTED) => Err(Error::new(
            ErrorCode::ProjectNotFound,
            format!("module {module} cannot be imported: {}", out.stderr.trim()),
        )),
        code => Err(Error::new(
            ErrorCode::Internal,
            format!("sidecar exited with {code:?}: {}", out.stderr.trim()),
        )),
    }
}

/// Convert sidecar output into the index's declaration records.
pub fn to_records(report: &SidecarReport) -> Vec<DeclRecord> {
    report
        .declarations
        .iter()
        .map(|d| DeclRecord {
            name: d.name.clone(),
            kind: d.kind.clone(),
            type_text: d.type_.clone(),
            // Lean versions differ in trailing whitespace on docstrings (4.35 keeps a final space,
            // 4.36 does not), so the stored text is trimmed at both ends.
            docstring: d.doc.as_deref().map(|t| t.trim().to_string()),
        })
        .collect()
}

/// Outcome of [`index_sidecar`]: which modules were stored, and why the others were not.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexSidecarStats {
    pub stored: Vec<String>,
    pub failed: Vec<(String, String)>,
}

/// Run the sidecar for every project module whose stored declarations are missing or stale, and
/// store the results in the index.
///
/// Lean can only import a project module once it has been built, so a module that is not built
/// is reported in `failed` (run `lake build` first). The toolchain key is `lean-<version>`, and
/// `LEAN_PATH` comes from `lake env printenv` so that the project's build outputs are found.
pub fn index_sidecar(
    index: &mut Index,
    project_root: &Path,
    exe: &Path,
    toolchain: &Toolchain,
    policy: &ProcessPolicy,
    cancel: &CancelToken,
) -> Result<IndexSidecarStats> {
    let version_out = crate::execute(toolchain, &crate::Command::LeanVersion, None, policy, cancel)?;
    let version = crate::parse_version(&version_out.output.stdout)
        .ok_or_else(|| Error::new(ErrorCode::LeanNotFound, "could not read the Lean version"))?;
    let tc_key = format!("lean-{version}");
    let sysroot = crate::lean_sysroot(toolchain, policy, cancel)?;

    let mut run_policy = policy.clone();
    if let Some(lean_path) = lean_path_for(toolchain, project_root, policy, cancel) {
        run_policy.extra_env.push(("LEAN_PATH".to_string(), lean_path));
    }

    let mut stats = IndexSidecarStats::default();
    for module in index.modules_needing_sidecar(&tc_key, SCHEMA_VERSION as i64)? {
        let outcome =
            run_sidecar(exe, &sysroot, &module, Some(project_root), &run_policy, cancel).and_then(|report| {
                index.store_declarations(&module, &tc_key, SCHEMA_VERSION as i64, &to_records(&report))
            });
        match outcome {
            Ok(()) => stats.stored.push(module),
            Err(e) => stats.failed.push((module, e.to_string())),
        }
    }
    Ok(stats)
}

/// `LEAN_PATH` as Lake sets it for the project, or `None` when Lake cannot report it.
pub fn lean_path_for(
    toolchain: &Toolchain,
    project_root: &Path,
    policy: &ProcessPolicy,
    cancel: &CancelToken,
) -> Option<String> {
    let out = crate::execute(
        toolchain,
        &crate::Command::LakeEnvPrintenv,
        Some(project_root),
        policy,
        cancel,
    )
    .ok()?;
    out.output
        .stdout
        .lines()
        .find_map(|l| l.strip_prefix("LEAN_PATH=").map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_report() {
        let text = r#"{"schema_version":1,"module":"M","declarations":[
            {"name":"f","kind":"definition","type":"Nat → Nat","doc":null},
            {"name":"g","kind":"theorem","type":"True","doc":"Docs."}]}"#;
        let r = parse_report(text).unwrap();
        assert_eq!(r.module, "M");
        assert_eq!(r.declarations.len(), 2);
        assert_eq!(r.declarations[0].type_, "Nat → Nat");
        assert_eq!(r.declarations[1].doc.as_deref(), Some("Docs."));
    }

    #[test]
    fn rejects_unknown_schema_version() {
        let text = r#"{"schema_version":2,"module":"M","declarations":[]}"#;
        assert!(parse_report(text).is_err());
    }

    #[test]
    fn rejects_invalid_json() {
        assert!(parse_report("not json").is_err());
    }

    #[test]
    fn module_names_are_validated() {
        assert!(validate_module_name("Init.Prelude").is_ok());
        assert!(validate_module_name("My_Lib.Sub-Mod'").is_ok());
        for bad in [
            "",
            ".Init",
            "Init.",
            "a..b",
            "Init/Prelude",
            "x y",
            "a;rm -rf",
            "\"q\"",
        ] {
            assert!(validate_module_name(bad).is_err(), "{bad:?} should be refused");
        }
    }
}
