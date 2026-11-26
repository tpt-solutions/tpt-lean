// SPDX-License-Identifier: MIT OR Apache-2.0
//! `tpt-lean fix`: mechanical fixes to one Lean file, planned as a reviewable diff.
//!
//! Three fixes are planned, in this order:
//!
//! 1. remove imports that nothing in the file uses (the `imports --unused` findings);
//! 2. add `import M` for an unknown identifier when exactly one project or indexed module declares
//!    it (the Lean diagnostics name the identifier; the index says where it is declared);
//! 3. format the result, when the file parses with certainty (`tpt-lean fmt`).
//!
//! [`plan_fix`] is pure and does the text work. [`fix_file`] gathers the inputs from the index and
//! from Lean, and optionally writes the result. Writing is checked: when Lean reports more errors
//! after the write than before, the original text is restored. Lean remains the authority.

use std::collections::BTreeSet;
use std::path::Path;

use serde::Serialize;
use tpt_lean_adapter::{self as adapter, Command as LeanCommand, Diagnostic, Severity};
use tpt_lean_core::paths::WorkspaceRoot;
use tpt_lean_core::policy::CancelToken;
use tpt_lean_core::{Error, Result};

use crate::imports::{suggest_imports, unused_imports};

/// One change in a plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FixStep {
    /// `remove_unused_import`, `add_import` or `format`.
    pub kind: &'static str,
    /// 1-based line in the original file, when the step has one.
    pub line: Option<usize>,
    /// What the step does, for a person.
    pub detail: String,
}

/// A planned edit to one file. Nothing is written by planning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FixPlan {
    /// Project-relative path with forward slashes.
    pub file: String,
    pub steps: Vec<FixStep>,
    /// Unified diff from the original to the fixed text (empty when there is nothing to do).
    pub diff: String,
    #[serde(skip)]
    pub original: String,
    #[serde(skip)]
    pub fixed: String,
}

/// The first `import` line of a file, or `None` when it has none. Used to place new imports.
fn last_import_line(src: &str) -> Option<usize> {
    tpt_lean_syntax::parse(src).imports.iter().map(|i| i.line).max()
}

/// Plan the fixes for `src`. `unused_lines` are 1-based import lines to remove; `add_modules` are
/// `(identifier, module)` pairs whose module should be imported. A module already imported is
/// skipped, and each module is added once.
pub fn plan_fix(
    file: &str,
    src: &str,
    unused_lines: &[usize],
    add_modules: &[(String, String)],
) -> Result<FixPlan> {
    let mut steps = Vec::new();
    let lines: Vec<&str> = src.lines().collect();
    let mut removed: BTreeSet<usize> = BTreeSet::new();
    for &line in unused_lines {
        if line >= 1 && line <= lines.len() && removed.insert(line) {
            steps.push(FixStep {
                kind: "remove_unused_import",
                line: Some(line),
                detail: format!("remove unused import: {}", lines[line - 1].trim()),
            });
        }
    }
    let mut kept: Vec<String> = lines
        .iter()
        .enumerate()
        .filter(|(i, _)| !removed.contains(&(i + 1)))
        .map(|(_, l)| (*l).to_string())
        .collect();

    let already: BTreeSet<String> = tpt_lean_syntax::parse(src)
        .imports
        .into_iter()
        .map(|i| i.module)
        .collect();
    let mut to_add: Vec<(String, String)> = Vec::new();
    for (name, module) in add_modules {
        if !already.contains(module) && !to_add.iter().any(|(_, m)| m == module) {
            to_add.push((name.clone(), module.clone()));
        }
    }
    if !to_add.is_empty() {
        // Insert after the last import that is kept, or at the top when there is none.
        let kept_text = kept.join("\n");
        let insert_at = last_import_line(&kept_text)
            .map(|l| l.min(kept.len()))
            .unwrap_or(0);
        let new_lines: Vec<String> = to_add
            .iter()
            .map(|(_, module)| format!("import {module}"))
            .collect();
        for (name, module) in &to_add {
            steps.push(FixStep {
                kind: "add_import",
                line: None,
                detail: format!("add `import {module}` for `{name}`"),
            });
        }
        kept.splice(insert_at..insert_at, new_lines);
    }

    let mut text = kept.join("\n");
    if src.ends_with('\n') {
        text.push('\n');
    }
    if let Ok(formatted) = tpt_lean_fmt::format_text(&text) {
        if formatted.changed {
            steps.push(FixStep {
                kind: "format",
                line: None,
                detail: "format the file".into(),
            });
            text = formatted.output;
        }
    }

    let diff = if text == src {
        String::new()
    } else {
        line_diff(file, src, &text)
    };
    Ok(FixPlan {
        file: file.to_string(),
        steps,
        diff,
        original: src.to_string(),
        fixed: text,
    })
}

/// A unified diff of two texts, with three lines of context. Empty when they are equal.
fn line_diff(file: &str, old: &str, new: &str) -> String {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    // Longest common subsequence over lines; files are small enough for the quadratic table.
    let mut table = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            table[i][j] = if a[i] == b[j] {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    // Walk the table into edit operations: ' ' same, '-' removed, '+' added.
    let (mut i, mut j) = (0, 0);
    let mut ops: Vec<(char, &str)> = Vec::new();
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            ops.push((' ', a[i]));
            i += 1;
            j += 1;
        } else if j < b.len() && (i == a.len() || table[i][j + 1] >= table[i + 1][j]) {
            ops.push(('+', b[j]));
            j += 1;
        } else {
            ops.push(('-', a[i]));
            i += 1;
        }
    }
    if ops.iter().all(|(c, _)| *c == ' ') {
        return String::new();
    }
    let mut out = format!("--- a/{file}\n+++ b/{file}\n");
    // Print each changed op with up to three unchanged lines around it.
    let changed: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter(|(_, (c, _))| *c != ' ')
        .map(|(k, _)| k)
        .collect();
    let mut shown = BTreeSet::new();
    for &k in &changed {
        let from = k.saturating_sub(3);
        let to = (k + 3).min(ops.len() - 1);
        for idx in from..=to {
            shown.insert(idx);
        }
    }
    let mut prev: Option<usize> = None;
    for idx in shown {
        if prev.is_some_and(|p| p + 1 != idx) {
            out.push_str("@@\n");
        }
        let (c, text) = ops[idx];
        out.push(c);
        out.push_str(text);
        out.push('\n');
        prev = Some(idx);
    }
    out
}

/// The result of [`fix_file`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FixOutcome {
    pub plan: FixPlan,
    /// Lean errors before any change.
    pub errors_before: usize,
    /// Identifiers that Lean reports as unknown but that no single module declares.
    pub unresolved: Vec<String>,
    /// True when the plan was written to disk.
    pub applied: bool,
    /// True when the write was undone because Lean reported more errors afterwards.
    pub rolled_back: bool,
    /// Lean errors after the write, when it was written.
    pub errors_after: Option<usize>,
}

/// Lean's diagnostics for `file` (absolute or relative to `project_root`) and the error count.
fn lean_errors(project_root: &Path, file: &Path, timeout_secs: u64) -> Result<(usize, Vec<String>)> {
    let tc = adapter::discover_toolchain();
    let policy = adapter::lean_policy(timeout_secs);
    let abs = if file.is_absolute() {
        file.to_path_buf()
    } else {
        project_root.join(file)
    };
    let result = adapter::execute(
        &tc,
        &LeanCommand::LeanCheck { file: abs },
        Some(project_root),
        &policy,
        &CancelToken::new(),
    )?;
    let errors: Vec<&Diagnostic> = result
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    let unknown = errors
        .iter()
        .filter(|d| d.message.starts_with("unknown identifier") || d.message.starts_with("unknown constant"))
        .filter_map(|d| d.message.split('\'').nth(1).map(String::from))
        .collect();
    Ok((errors.len(), unknown))
}

/// Plan, and optionally apply, the fixes for one file of the project. Lean is run on the file
/// before and, when `apply` is set and something changed, after the write. A write that leaves more
/// Lean errors than before is undone.
pub fn fix_file(project_root: &Path, file: &Path, timeout_secs: u64, apply: bool) -> Result<FixOutcome> {
    let ws = WorkspaceRoot::new(project_root)?;
    let target = ws.resolve(&file.to_string_lossy())?;
    let rel = crate::relative_display(ws.path(), &target);
    let src = std::fs::read_to_string(&target).map_err(|e| Error::from(e).with_path(&target))?;

    let unused: Vec<usize> = unused_imports(project_root, &target)?
        .iter()
        .filter(|f| f.code == "import_unused")
        .map(|f| f.line)
        .collect();
    let (errors_before, unknown) = lean_errors(project_root, &target, timeout_secs)?;

    let mut add: Vec<(String, String)> = Vec::new();
    let mut unresolved = Vec::new();
    let mut seen = BTreeSet::new();
    for name in unknown {
        if !seen.insert(name.clone()) {
            continue;
        }
        let modules: Vec<String> = suggest_imports(project_root, &name, 20)?
            .into_iter()
            .filter(|s| s.examples.iter().any(|e| *e == name))
            .map(|s| s.module)
            .collect();
        match modules.as_slice() {
            [only] => add.push((name, only.clone())),
            _ => unresolved.push(name),
        }
    }

    let plan = plan_fix(&rel, &src, &unused, &add)?;
    let mut outcome = FixOutcome {
        plan,
        errors_before,
        unresolved,
        applied: false,
        rolled_back: false,
        errors_after: None,
    };
    if apply && outcome.plan.fixed != src {
        std::fs::write(&target, &outcome.plan.fixed).map_err(|e| Error::from(e).with_path(&target))?;
        let (after, _) = lean_errors(project_root, &target, timeout_secs)?;
        outcome.errors_after = Some(after);
        if after > errors_before {
            std::fs::write(&target, &src).map_err(|e| Error::from(e).with_path(&target))?;
            outcome.rolled_back = true;
        } else {
            outcome.applied = true;
        }
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_unused_imports_and_keeps_the_rest() {
        let src = "import Foo\nimport Bar\n\ndef x := 1\n";
        let plan = plan_fix("A.lean", src, &[1], &[]).unwrap();
        assert_eq!(plan.fixed, "import Bar\n\ndef x := 1\n");
        assert_eq!(plan.steps[0].kind, "remove_unused_import");
        assert_eq!(plan.steps[0].line, Some(1));
        assert!(plan.diff.contains("-import Foo"));
    }

    #[test]
    fn adds_a_missing_import_after_the_last_import_once() {
        let src = "import Bar\n\ndef x := 1\n";
        let add = vec![
            ("Baz.f".to_string(), "Baz".to_string()),
            ("Baz.g".to_string(), "Baz".to_string()),
        ];
        let plan = plan_fix("A.lean", src, &[], &add).unwrap();
        assert_eq!(plan.fixed, "import Bar\nimport Baz\n\ndef x := 1\n");
        // Two identifiers from one module add that module once.
        assert_eq!(plan.steps.iter().filter(|s| s.kind == "add_import").count(), 1);
    }

    #[test]
    fn does_not_add_a_module_that_is_already_imported() {
        let src = "import Baz\n\ndef x := 1\n";
        let add = vec![("Baz.f".to_string(), "Baz".to_string())];
        let plan = plan_fix("A.lean", src, &[], &add).unwrap();
        assert_eq!(plan.fixed, src);
        assert!(plan.steps.is_empty());
        assert!(plan.diff.is_empty());
    }

    #[test]
    fn a_file_without_imports_gets_the_import_at_the_top() {
        let src = "def x := 1\n";
        let add = vec![("Baz.f".to_string(), "Baz".to_string())];
        let plan = plan_fix("A.lean", src, &[], &add).unwrap();
        assert!(plan.fixed.starts_with("import Baz\n"));
    }

    #[test]
    fn nothing_to_do_gives_an_empty_plan() {
        let src = "import Bar\n\ndef x := 1\n";
        let plan = plan_fix("A.lean", src, &[], &[]).unwrap();
        assert_eq!(plan.fixed, src);
        assert!(plan.steps.is_empty());
    }
}
