// SPDX-License-Identifier: MIT OR Apache-2.0
//! Client for the Lean sidecar (`lean/tpt-lean-sidecar`), level 3 of the adapter.
//!
//! The sidecar is a small Lean program that imports one module through Lean's own environment
//! and prints its declarations (name, kind, pretty-printed type, docstring) as one JSON object.
//! This module runs it under the shared process policy and parses the result. The JSON schema
//! is versioned; an unknown version is rejected rather than guessed at.

use crate::runner::{run, ProcessOutput};
use crate::Toolchain;
use serde::{Deserialize, Serialize};
use std::path::Path;
use tpt_lean_core::policy::{CancelToken, ProcessPolicy};
use tpt_lean_core::{Error, ErrorCode, Result};
use tpt_lean_index::{DeclRecord, Index};

/// The sidecar JSON schema version this client understands.
pub const SCHEMA_VERSION: u64 = 1;

/// Exit code the sidecar uses when the module cannot be imported.
const EXIT_MODULE_NOT_IMPORTED: i32 = 3;
/// Exit code the sidecar uses when a source file cannot be read or elaborated (goals mode).
const EXIT_FILE_NOT_ELABORATED: i32 = 4;

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
/// The policy for a sidecar run: `LEAN_SYSROOT` for Lean's library, and the toolchain's `bin`
/// directory first on `PATH`. The sidecar links Lean's shared library, which the loader finds
/// through `PATH` on Windows.
fn sidecar_policy(policy: &ProcessPolicy, sysroot: &Path, lean_path: Option<&str>) -> ProcessPolicy {
    let mut policy = policy.clone();
    policy
        .extra_env
        .push(("LEAN_SYSROOT".to_string(), sysroot.display().to_string()));
    let sep = if cfg!(windows) { ";" } else { ":" };
    let current = std::env::var("PATH").unwrap_or_default();
    policy.extra_env.push((
        "PATH".to_string(),
        format!("{}{sep}{current}", sysroot.join("bin").display()),
    ));
    if let Some(lean_path) = lean_path {
        policy
            .extra_env
            .push(("LEAN_PATH".to_string(), lean_path.to_string()));
    }
    policy
}

pub fn run_sidecar(
    exe: &Path,
    sysroot: &Path,
    module: &str,
    cwd: Option<&Path>,
    policy: &ProcessPolicy,
    cancel: &CancelToken,
) -> Result<SidecarReport> {
    let module = validate_module_name(module)?;
    let policy = sidecar_policy(policy, sysroot, None);
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

/// Goals before the innermost tactic at a 1-based `line` and `column` of `file`, from the
/// sidecar's `goals` mode. `file` is resolved against `cwd`, the project root. `lean_path` is the
/// project's search path (see [`lean_path_for`]) so that project imports resolve. Empty when no
/// tactic is at the position.
pub fn run_sidecar_goals(
    exe: &Path,
    sysroot: &Path,
    site: &GoalSite<'_>,
    cwd: Option<&Path>,
    lean_path: Option<&str>,
    policy: &ProcessPolicy,
    cancel: &CancelToken,
) -> Result<Vec<String>> {
    run_sidecar_goals_report(exe, sysroot, site, cwd, lean_path, policy, cancel)
        .map(|report| report.goals)
}

/// Like [`run_sidecar_goals`], but returns the whole report, including what kind of position it is
/// (see [`GoalTarget`]).
pub fn run_sidecar_goals_report(
    exe: &Path,
    sysroot: &Path,
    site: &GoalSite<'_>,
    cwd: Option<&Path>,
    lean_path: Option<&str>,
    policy: &ProcessPolicy,
    cancel: &CancelToken,
) -> Result<GoalsReport> {
    let policy = sidecar_policy(policy, sysroot, lean_path);
    let file = site.file;
    let args = vec![
        "goals".to_string(),
        file.display().to_string(),
        site.line.to_string(),
        site.column.to_string(),
    ];
    let out: ProcessOutput = run(exe, &args, cwd, &policy, cancel)?;
    if out.timed_out || out.cancelled || out.memory_exceeded || out.output_truncated {
        return Err(Error::new(
            ErrorCode::Timeout,
            format!(
                "sidecar did not finish for goals at {}:{}:{} (timed_out={}, cancelled={})",
                file.display(),
                site.line,
                site.column,
                out.timed_out,
                out.cancelled
            ),
        ));
    }
    match out.exit_code {
        Some(0) => {
            let report: GoalsReport = serde_json::from_str(out.stdout.trim()).map_err(|e| {
                Error::new(
                    ErrorCode::Internal,
                    format!("sidecar goals output is not valid JSON: {e}"),
                )
            })?;
            if !GOALS_SCHEMA_VERSIONS.contains(&report.schema_version) {
                return Err(Error::new(
                    ErrorCode::Internal,
                    format!(
                        "sidecar goals schema version {} is not supported",
                        report.schema_version
                    ),
                ));
            }
            Ok(report)
        }
        Some(EXIT_FILE_NOT_ELABORATED) => Err(Error::new(
            ErrorCode::Io,
            format!("cannot get goals in {}: {}", file.display(), out.stderr.trim()),
        )),
        code => Err(Error::new(
            ErrorCode::Internal,
            format!("sidecar goals exited with {code:?}: {}", out.stderr.trim()),
        )),
    }
}

/// Goals at several positions of one file, from one elaboration of the file (the sidecar's
/// `sites` mode). The result is in the order of `sites`, which are 1-based `(line, column)` pairs.
#[allow(clippy::too_many_arguments)]
pub fn run_sidecar_sites(
    exe: &Path,
    sysroot: &Path,
    file: &Path,
    sites: &[(u64, u64)],
    cwd: Option<&Path>,
    lean_path: Option<&str>,
    policy: &ProcessPolicy,
    cancel: &CancelToken,
) -> Result<Vec<SiteGoals>> {
    if sites.is_empty() {
        return Ok(Vec::new());
    }
    let policy = sidecar_policy(policy, sysroot, lean_path);
    let mut args = vec!["sites".to_string(), file.display().to_string()];
    args.extend(sites.iter().map(|(line, column)| format!("{line}:{column}")));
    let out: ProcessOutput = run(exe, &args, cwd, &policy, cancel)?;
    if out.timed_out || out.cancelled || out.memory_exceeded || out.output_truncated {
        return Err(Error::new(
            ErrorCode::Timeout,
            format!(
                "sidecar did not finish for goals in {} (timed_out={}, cancelled={})",
                file.display(),
                out.timed_out,
                out.cancelled
            ),
        ));
    }
    match out.exit_code {
        Some(0) => {
            let report: SitesReport = serde_json::from_str(out.stdout.trim()).map_err(|e| {
                Error::new(
                    ErrorCode::Internal,
                    format!("sidecar sites output is not valid JSON: {e}"),
                )
            })?;
            if !GOALS_SCHEMA_VERSIONS.contains(&report.schema_version) {
                return Err(Error::new(
                    ErrorCode::Internal,
                    format!(
                        "sidecar sites schema version {} is not supported",
                        report.schema_version
                    ),
                ));
            }
            if report.sites.len() != sites.len() {
                return Err(Error::new(
                    ErrorCode::Internal,
                    format!(
                        "sidecar answered {} site(s) for {} requested",
                        report.sites.len(),
                        sites.len()
                    ),
                ));
            }
            Ok(report.sites)
        }
        Some(EXIT_FILE_NOT_ELABORATED) => Err(Error::new(
            ErrorCode::Io,
            format!("cannot get goals in {}: {}", file.display(), out.stderr.trim()),
        )),
        code => Err(Error::new(
            ErrorCode::Internal,
            format!("sidecar sites exited with {code:?}: {}", out.stderr.trim()),
        )),
    }
}

/// A position in a source file for the goals mode: the file (relative to the project root) and a
/// 1-based line and 1-based column, counting characters.
#[derive(Debug, Clone, Copy)]
pub struct GoalSite<'a> {
    pub file: &'a Path,
    pub line: u64,
    pub column: u64,
}

/// Goals-mode schema versions this client reads. Version 1 had tactic goals only.
pub const GOALS_SCHEMA_VERSIONS: [u64; 2] = [1, 2];

/// What a goals position is about. Version 1 reports carry no target and are tactic goals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GoalTarget {
    /// Goals before the innermost tactic containing the position.
    #[default]
    Tactic,
    /// The expected type of the innermost term containing the position.
    Term,
    /// The type of the declaration whose name contains the position.
    Statement,
    /// Nothing at the position has goals.
    #[serde(rename = "none")]
    Nothing,
}

/// Output of the sidecar's `goals` mode.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct GoalsReport {
    pub schema_version: u64,
    pub file: String,
    pub line: u64,
    pub column: u64,
    #[serde(default)]
    pub target: GoalTarget,
    pub goals: Vec<String>,
}

/// Goals at one position, from the sidecar's `sites` mode.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct SiteGoals {
    pub line: u64,
    pub column: u64,
    pub target: GoalTarget,
    pub goals: Vec<String>,
}

/// Output of the sidecar's `sites` mode.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct SitesReport {
    schema_version: u64,
    sites: Vec<SiteGoals>,
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
    fn goals_v1_reports_are_tactic_goals() {
        let text = r#"{"schema_version":1,"file":"M.lean","line":2,"column":3,"goals":["⊢ True"]}"#;
        let r: GoalsReport = serde_json::from_str(text).unwrap();
        assert_eq!(r.target, GoalTarget::Tactic);
        assert_eq!(r.goals, vec!["⊢ True"]);
    }

    #[test]
    fn goals_v2_reports_carry_their_target() {
        for (wire, expected) in [
            ("tactic", GoalTarget::Tactic),
            ("term", GoalTarget::Term),
            ("statement", GoalTarget::Statement),
            ("none", GoalTarget::Nothing),
        ] {
            let text = format!(
                r#"{{"schema_version":2,"file":"M.lean","line":1,"column":1,"target":"{wire}","goals":[]}}"#
            );
            let r: GoalsReport = serde_json::from_str(&text).unwrap();
            assert_eq!(r.target, expected, "target {wire}");
        }
    }

    #[test]
    fn sites_reports_are_read_in_order() {
        let text = r#"{"schema_version":2,"file":"M.lean","sites":[
            {"line":2,"column":3,"target":"tactic","goals":["⊢ True"]},
            {"line":4,"column":58,"target":"term","goals":["p : Prop","⊢ p"]}]}"#;
        let r: SitesReport = serde_json::from_str(text).unwrap();
        assert_eq!(r.sites.len(), 2);
        assert_eq!(r.sites[1].target, GoalTarget::Term);
        assert_eq!(r.sites[1].goals, vec!["p : Prop", "⊢ p"]);
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
