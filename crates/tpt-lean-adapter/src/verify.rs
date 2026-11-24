// SPDX-License-Identifier: MIT OR Apache-2.0
//! Verification of formatter output with the real Lean checker.
//!
//! Lean stays the source of truth: a formatted file is accepted only when Lean reports no
//! error that the original did not already have. The original and the formatted text are
//! written side by side into a temporary directory under the project's `.tpt-lean/tmp`, then
//! both are checked with `lean` from the project root, so relative imports resolve the same
//! way for both. Error messages are compared with positions removed, because formatting
//! legitimately moves lines and columns.

use crate::diagnostics::{Diagnostic, Severity};
use crate::{execute, Command, Toolchain};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tpt_lean_core::policy::{CancelToken, ProcessPolicy};
use tpt_lean_core::{Error, ErrorCode, Result};

/// Outcome of comparing Lean's verdict on the original and formatted text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatVerification {
    /// True when the formatted text introduces no new Lean errors.
    pub verified: bool,
    /// Error messages present in the formatted text but not in the original.
    pub new_errors: Vec<String>,
    /// Number of error diagnostics in the original text.
    pub original_errors: usize,
    /// Number of error diagnostics in the formatted text.
    pub formatted_errors: usize,
}

/// Check `original` and `formatted` with `lean` and report whether formatting kept Lean's verdict.
///
/// Returns an error (not `verified: false`) when Lean could not be run to completion on either
/// side, because then nothing can be concluded: a timeout, cancellation, memory overrun, or
/// truncated output is not evidence either way.
pub fn verify_format(
    toolchain: &Toolchain,
    project_root: &Path,
    original: &str,
    formatted: &str,
    policy: &ProcessPolicy,
    cancel: &CancelToken,
) -> Result<FormatVerification> {
    if toolchain.lean.is_none() {
        return Err(Error::new(
            ErrorCode::LeanNotFound,
            "lean not found; cannot verify",
        ));
    }
    let dir = scratch_dir(project_root)?;
    let result = check_both(toolchain, project_root, &dir, original, formatted, policy, cancel);
    // Best-effort cleanup; a leftover temp directory is harmless and is cleared by `clean`.
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn check_both(
    toolchain: &Toolchain,
    project_root: &Path,
    dir: &Path,
    original: &str,
    formatted: &str,
    policy: &ProcessPolicy,
    cancel: &CancelToken,
) -> Result<FormatVerification> {
    let orig_path = dir.join("original.lean");
    let fmt_path = dir.join("formatted.lean");
    std::fs::write(&orig_path, original).map_err(|e| Error::from(e).with_path(&orig_path))?;
    std::fs::write(&fmt_path, formatted).map_err(|e| Error::from(e).with_path(&fmt_path))?;

    let before = check_file(toolchain, project_root, &orig_path, policy, cancel)?;
    let after = check_file(toolchain, project_root, &fmt_path, policy, cancel)?;

    let before_counts = error_counts(&before);
    let after_counts = error_counts(&after);
    let mut new_errors = Vec::new();
    for (message, count) in &after_counts {
        let had = before_counts.get(message).copied().unwrap_or(0);
        for _ in had..*count {
            new_errors.push(message.clone());
        }
    }
    let original_errors = before.iter().filter(|d| d.severity == Severity::Error).count();
    let formatted_errors = after.iter().filter(|d| d.severity == Severity::Error).count();
    Ok(FormatVerification {
        verified: new_errors.is_empty(),
        new_errors,
        original_errors,
        formatted_errors,
    })
}

/// Run `lean` on one file. Fails if the run was cut short, since a partial run proves nothing.
fn check_file(
    toolchain: &Toolchain,
    project_root: &Path,
    file: &Path,
    policy: &ProcessPolicy,
    cancel: &CancelToken,
) -> Result<Vec<Diagnostic>> {
    let result = execute(
        toolchain,
        &Command::LeanCheck {
            file: file.to_path_buf(),
        },
        Some(project_root),
        policy,
        cancel,
    )?;
    let out = &result.output;
    if out.timed_out || out.cancelled || out.memory_exceeded || out.output_truncated {
        return Err(Error::new(
            ErrorCode::Io,
            format!(
                "lean did not finish checking {} (timed_out={}, cancelled={}, memory_exceeded={}, output_truncated={})",
                file.display(),
                out.timed_out,
                out.cancelled,
                out.memory_exceeded,
                out.output_truncated
            ),
        ));
    }
    Ok(result.diagnostics)
}

/// Multiset of error messages with positions removed.
fn error_counts(diags: &[Diagnostic]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for d in diags.iter().filter(|d| d.severity == Severity::Error) {
        *counts.entry(d.message.trim().to_string()).or_insert(0) += 1;
    }
    counts
}

/// A fresh directory under `<project>/.tpt-lean/tmp`, unique per call.
fn scratch_dir(project_root: &Path) -> Result<PathBuf> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = project_root
        .join(".tpt-lean")
        .join("tmp")
        .join(format!("verify-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| Error::from(e).with_path(&dir))?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover_toolchain;

    fn project() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tpt-verify-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn unchanged_formatting_is_verified() {
        let tc = discover_toolchain();
        if tc.lean.is_none() {
            eprintln!("skipped: lean not installed");
            return;
        }
        let root = project();
        let src = "def x : Nat := 1\n";
        let v = verify_format(
            &tc,
            &root,
            src,
            src,
            &ProcessPolicy::default(),
            &CancelToken::new(),
        )
        .unwrap();
        assert!(v.verified, "{v:?}");
        assert_eq!(v.original_errors, v.formatted_errors);
    }

    #[test]
    fn formatting_that_breaks_the_proof_is_refused() {
        let tc = discover_toolchain();
        if tc.lean.is_none() {
            eprintln!("skipped: lean not installed");
            return;
        }
        let root = project();
        let original = "def x : Nat := 1\n";
        let broken = "def x : Nat := \"not a nat\"\n";
        let v = verify_format(
            &tc,
            &root,
            original,
            broken,
            &ProcessPolicy::default(),
            &CancelToken::new(),
        )
        .unwrap();
        assert!(!v.verified, "{v:?}");
        assert!(!v.new_errors.is_empty());
    }

    #[test]
    fn pre_existing_errors_do_not_block_verification() {
        let tc = discover_toolchain();
        if tc.lean.is_none() {
            eprintln!("skipped: lean not installed");
            return;
        }
        let root = project();
        let original = "def y : Nat := \"bad\"\n";
        let reformatted = "def y : Nat :=   \"bad\"\n";
        let v = verify_format(
            &tc,
            &root,
            original,
            reformatted,
            &ProcessPolicy::default(),
            &CancelToken::new(),
        )
        .unwrap();
        assert!(v.verified, "{v:?}");
        assert!(v.original_errors > 0);
    }
}
