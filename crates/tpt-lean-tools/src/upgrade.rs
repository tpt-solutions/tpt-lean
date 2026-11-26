// SPDX-License-Identifier: MIT OR Apache-2.0
//! `tpt-lean upgrade --to VERSION`: move a project to another Lean release.
//!
//! The plan is pure text work: the new `lean-toolchain` line, whether the target is one this
//! release has been tested against, and notes for the parts this tool does not change by itself
//! (the Mathlib `require` tag and `lake-manifest.json`). Applying writes `lean-toolchain`, runs
//! `lake build` through elan (which installs the toolchain), and restores the original file when
//! the build fails. Lean remains the authority on whether the project still builds.

use std::path::Path;

use serde::Serialize;
use tpt_lean_adapter::compat::{self, Compatibility};
use tpt_lean_adapter::{self as adapter, Command as LeanCommand, Severity};
use tpt_lean_core::policy::CancelToken;
use tpt_lean_core::{Error, Result};

/// A planned change to the project's Lean toolchain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpgradePlan {
    /// The current `lean-toolchain` line, trimmed.
    pub from: String,
    /// The `lean-toolchain` line the plan writes, such as `leanprover/lean4:v4.37.0`.
    pub to: String,
    /// `from` and `to` are the same, so nothing would change.
    pub unchanged: bool,
    /// How the target compares with the Lean versions this release is tested against.
    pub compatibility: &'static str,
    /// The changes the plan makes, in order.
    pub steps: Vec<String>,
    /// Things the user must check or change by hand.
    pub notes: Vec<String>,
}

/// What [`apply_upgrade`] did.
#[derive(Debug, Clone, Serialize)]
pub struct UpgradeOutcome {
    pub plan: UpgradePlan,
    /// The toolchain file was written and the build succeeded.
    pub applied: bool,
    /// The build failed and the original `lean-toolchain` was restored.
    pub rolled_back: bool,
    /// Lean errors reported by the build that ran after the change.
    pub build_errors: usize,
    /// Exit status of `lake build`, when it ran to completion.
    pub build_exit: Option<i32>,
    /// The first `error` line of the build's output, or its last line: the likeliest reason a
    /// build failed (for example, a Lean error or that elan could not install the toolchain).
    pub build_message: Option<String>,
}

/// Parse a target such as `4.37.0`, `v4.37.0` or `4.37` into the full `lean-toolchain` line.
pub fn toolchain_line(target: &str) -> Result<String> {
    let triple = compat::version_triple(target).ok_or_else(|| {
        Error::invalid(format!(
            "--to needs a Lean release such as 4.37.0 or v4.37.0, not {target:?}"
        ))
    })?;
    Ok(format!(
        "leanprover/lean4:v{}.{}.{}",
        triple.0, triple.1, triple.2
    ))
}

/// Plan the upgrade from the current `lean-toolchain` text and the lakefile text.
///
/// `lakefile` may be empty. Its Mathlib pin, if any, is reported and never edited.
pub fn plan_upgrade(current_toolchain: &str, lakefile: &str, target: &str) -> Result<UpgradePlan> {
    let to = toolchain_line(target)?;
    let from = current_toolchain.trim().to_string();
    let version = compat::pinned_version(&to).unwrap_or_default();
    let compatibility = compat::classify(&version);
    let unchanged = from == to;

    let mut steps = Vec::new();
    let mut notes = Vec::new();
    if unchanged {
        steps.push(format!("lean-toolchain already pins {to}; nothing to change"));
    } else {
        steps.push(format!("lean-toolchain: {from} -> {to}"));
        steps.push("lake build (elan installs the toolchain if it is missing)".to_string());
    }

    match compatibility {
        Compatibility::Tested => {}
        other => notes.push(format!(
            "{} is {} relative to the Lean versions this tpt-lean release is tested against; features may behave differently",
            version,
            other.as_str()
        )),
    }

    if let Some(tag) = mathlib_tag(lakefile) {
        notes.push(format!(
            "lakefile.lean pins Mathlib at \"{tag}\"; this is not changed. Set it to the Mathlib tag for {to} once that tag exists, then run lake update"
        ));
    }
    notes.push(
        "lake-manifest.json is not changed; run `lake update` if dependencies need new revisions".to_string(),
    );

    Ok(UpgradePlan {
        from,
        to,
        unchanged,
        compatibility: compatibility.as_str(),
        steps,
        notes,
    })
}

/// The quoted tag on a `require mathlib ... @ "TAG"` line, if the lakefile has one.
fn mathlib_tag(lakefile: &str) -> Option<String> {
    lakefile
        .lines()
        .filter(|l| l.contains("mathlib") || l.contains("Mathlib"))
        .find_map(|l| {
            let after = l.split("@ \"").nth(1)?;
            let tag = after.split('"').next()?;
            (!tag.is_empty()).then(|| tag.to_string())
        })
}

/// Read the current toolchain and lakefile under `root`, then plan the upgrade.
pub fn plan_for_project(root: &Path, target: &str) -> Result<UpgradePlan> {
    let current = read_toolchain(root)?;
    let lakefile = std::fs::read_to_string(root.join("lakefile.lean"))
        .or_else(|_| std::fs::read_to_string(root.join("lakefile.toml")))
        .unwrap_or_default();
    plan_upgrade(&current, &lakefile, target)
}

fn read_toolchain(root: &Path) -> Result<String> {
    let path = root.join("lean-toolchain");
    let text = std::fs::read_to_string(&path).map_err(|e| Error::from(e).with_path(&path))?;
    if text.trim().is_empty() {
        return Err(Error::invalid(format!("{} is empty", path.display())));
    }
    Ok(text)
}

/// Apply the plan: write `lean-toolchain`, run `lake build`, and restore the original text when
/// the build fails or is cancelled. A plan with no change does nothing and does not build.
pub fn apply_upgrade(root: &Path, plan: UpgradePlan, timeout_secs: u64) -> Result<UpgradeOutcome> {
    if plan.unchanged {
        return Ok(UpgradeOutcome {
            plan,
            applied: false,
            rolled_back: false,
            build_errors: 0,
            build_exit: None,
            build_message: None,
        });
    }
    let path = root.join("lean-toolchain");
    let original = read_toolchain(root)?;
    if original.trim() != plan.from {
        return Err(Error::invalid(
            "lean-toolchain changed since the plan was made; run upgrade again",
        ));
    }
    std::fs::write(&path, format!("{}\n", plan.to)).map_err(|e| Error::from(e).with_path(&path))?;

    let tc = adapter::discover_toolchain();
    let policy = adapter::lean_policy(timeout_secs);
    let run = adapter::execute(
        &tc,
        &LeanCommand::LakeBuild { target: None },
        Some(root),
        &policy,
        &CancelToken::new(),
    );
    let (ok, errors, exit, message) = match &run {
        Ok(r) => (
            r.output.success(),
            r.diagnostics
                .iter()
                .filter(|d| d.severity == Severity::Error)
                .count(),
            r.output.exit_code,
            failure_line(&format!(
                "{}
{}",
                r.output.stdout, r.output.stderr
            )),
        ),
        Err(e) => (false, 0, None, Some(e.to_string())),
    };

    if ok {
        return Ok(UpgradeOutcome {
            plan,
            applied: true,
            rolled_back: false,
            build_errors: errors,
            build_exit: exit,
            build_message: message,
        });
    }
    std::fs::write(&path, &original).map_err(|e| Error::from(e).with_path(&path))?;
    run?;
    Ok(UpgradeOutcome {
        plan,
        applied: false,
        rolled_back: true,
        build_errors: errors,
        build_exit: exit,
        build_message: message,
    })
}

/// The first line that starts with `error`, or else the last non-empty line.
fn failure_line(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    lines
        .iter()
        .find(|l| l.starts_with("error"))
        .or_else(|| lines.last())
        .map(|l| l.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_accepts_common_spellings() {
        assert_eq!(toolchain_line("4.37.0").unwrap(), "leanprover/lean4:v4.37.0");
        assert_eq!(toolchain_line("v4.37").unwrap(), "leanprover/lean4:v4.37.0");
        assert!(toolchain_line("stable").is_err());
        assert!(toolchain_line("4.x").is_err());
    }

    #[test]
    fn plan_writes_new_toolchain_and_flags_untested_target() {
        let plan = plan_upgrade("leanprover/lean4:v4.36.0\n", "", "4.37.0").unwrap();
        assert_eq!(plan.to, "leanprover/lean4:v4.37.0");
        assert!(!plan.unchanged);
        assert_eq!(plan.compatibility, "newer");
        assert!(plan.steps[0].contains("v4.36.0 -> leanprover/lean4:v4.37.0"));
        assert!(plan.notes.iter().any(|n| n.contains("newer")));
    }

    #[test]
    fn tested_target_has_no_compatibility_note() {
        let plan = plan_upgrade("leanprover/lean4:v4.35.0", "", "4.36.0").unwrap();
        assert_eq!(plan.compatibility, "tested");
        assert!(!plan.notes.iter().any(|n| n.contains("tested against")));
    }

    #[test]
    fn same_version_is_unchanged() {
        let plan = plan_upgrade("leanprover/lean4:v4.36.0\n", "", "v4.36.0").unwrap();
        assert!(plan.unchanged);
    }

    #[test]
    fn mathlib_pin_is_reported_not_edited() {
        let lakefile =
            "require mathlib from git \"https://github.com/leanprover-community/mathlib4\" @ \"v4.36.0\"\n";
        let plan = plan_upgrade("leanprover/lean4:v4.36.0", lakefile, "4.37.0").unwrap();
        assert!(plan
            .notes
            .iter()
            .any(|n| n.contains("\"v4.36.0\"") && n.contains("not changed")));
    }

    #[test]
    fn rejects_invalid_target() {
        assert!(plan_upgrade("leanprover/lean4:v4.36.0", "", "latest").is_err());
    }
}
