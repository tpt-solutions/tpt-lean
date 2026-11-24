// SPDX-License-Identifier: MIT OR Apache-2.0
//! End-to-end checks against the installed Lean toolchain. Skipped (with a message) when `lean`
//! cannot be found, so the suite still runs on machines without Lean.

use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use tpt_lean_profile::{compare, from_json, profile_file, to_json};

struct TempProject(PathBuf);

impl TempProject {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path =
            std::env::temp_dir().join(format!("tpt-lean-profile-test-{tag}-{}-{nanos}", process::id()));
        fs::create_dir_all(&path).expect("create temp project");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn lean_available() -> bool {
    let found = tpt_lean_adapter::discover_toolchain().lean.is_some();
    if !found {
        eprintln!("skipping: lean not found");
    }
    found
}

const GOOD: &str = "class Shape (α : Type) where\n  area : α → Nat\n\n\
instance : Shape Nat := ⟨fun n => n * n⟩\n\n\
theorem sumlemma (xs : List Nat) : (xs ++ []).length = xs.length := by\n  simp\n\n\
def bigsum : Nat := (List.range 200).foldl (· + ·) 0\n\n\
theorem heavy (a b c d : Nat) (h : a < b) (h2 : c < d) : a + c < b + d := by\n  omega\n\n\
example : 2 + 2 = 4 := by decide\n";

#[test]
fn profiles_a_clean_file() {
    if !lean_available() {
        return;
    }
    let project = TempProject::new("clean");
    let file = project.path().join("Sample.lean");
    fs::write(&file, GOOD).unwrap();

    let report = profile_file(project.path(), &file, 120).expect("profile succeeds");
    assert_eq!(report.lean_exit_code, Some(0));
    assert_eq!(report.lean_errors, 0);
    assert!(report.wall_ms > 0.0);
    assert!(report.declared_ms > 0.0);

    let sumlemma = report
        .declarations
        .iter()
        .find(|d| d.name.as_deref() == Some("sumlemma"))
        .expect("sumlemma reported");
    assert_eq!(sumlemma.kind, "theorem");
    assert_eq!(sumlemma.line, 6, "line in the original file");
    assert!(sumlemma.total_ms > 0.0);
    assert!(report.categories.contains_key("elaboration"));
    assert!(!report.notes.is_empty());

    let json = to_json(&report).unwrap();
    assert_eq!(from_json(&json).unwrap(), report);
    let text = compare(&report, &report);
    assert!(text.contains("Wall time:"));
}

#[test]
fn reports_errors_without_failing() {
    if !lean_available() {
        return;
    }
    let project = TempProject::new("errors");
    let file = project.path().join("Broken.lean");
    fs::write(&file, "theorem bad : 1 = 2 := by decide\n").unwrap();

    let report = profile_file(project.path(), &file, 120).expect("profile still returns a report");
    assert_ne!(report.lean_exit_code, Some(0));
    assert!(report.lean_errors >= 1);
    assert!(report.suggestions.iter().any(|s| s.contains("error")));
}

#[test]
fn missing_file_is_an_error() {
    let project = TempProject::new("missing");
    let err = profile_file(project.path(), &project.path().join("Nope.lean"), 10).unwrap_err();
    assert_eq!(err.code(), tpt_lean_core::ErrorCode::Io);
}

#[test]
fn zero_timeout_reports_timeout() {
    if !lean_available() {
        return;
    }
    let project = TempProject::new("timeout");
    let file = project.path().join("Slow.lean");
    fs::write(&file, GOOD).unwrap();
    let err = profile_file(project.path(), &file, 0).unwrap_err();
    assert_eq!(err.code(), tpt_lean_core::ErrorCode::Timeout);
}
