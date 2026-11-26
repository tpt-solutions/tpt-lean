// SPDX-License-Identifier: MIT OR Apache-2.0
//! Finds `sorry` and `admit` in Lean sources, per declaration.
//!
//! The scan works on lexer tokens, so a word inside a comment or a string is never reported. A
//! hit is attributed to the nearest declaration above it, as the syntax layer finds them. The scan
//! does not elaborate the file, so it cannot see a `sorry` that a macro introduces. Goals are not
//! part of the scan; [`attach_goals`] adds them from the Lean sidecar when asked. Lean remains the
//! authority on whether a proof is complete.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;
use tpt_lean_adapter::sidecar::SiteGoals;
use tpt_lean_core::paths::WorkspaceRoot;
use tpt_lean_core::Result;
use tpt_lean_syntax::lexer::TokenKind;

use crate::relative_display;

/// One `sorry` or `admit` in a source file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SorryHit {
    /// Project-relative path with forward slashes.
    pub file: String,
    /// 1-based line.
    pub line: usize,
    /// 1-based column in characters.
    pub column: usize,
    /// `sorry` or `admit`.
    pub keyword: &'static str,
    /// The declaration the hit is in, or `null` when it is outside any declaration.
    pub declaration: Option<String>,
    /// The goal at the hit, when the sidecar was asked for goals (`sorry-report --sidecar`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goals: Option<SiteGoals>,
    /// Why the sidecar gave no goal for this hit's file, when it was asked and failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goals_error: Option<String>,
}

/// Attach the goal at each hit. `fetch` answers the positions of one file (1-based line and column,
/// in the order given); the CLI backs it with the Lean sidecar. When a file cannot be answered, its
/// hits get `goals_error` and the other files are still answered.
pub fn attach_goals<F>(hits: &mut [SorryHit], mut fetch: F)
where
    F: FnMut(&str, &[(u64, u64)]) -> Result<Vec<SiteGoals>>,
{
    let files: BTreeSet<String> = hits.iter().map(|h| h.file.clone()).collect();
    for file in files {
        let indices: Vec<usize> = hits
            .iter()
            .enumerate()
            .filter(|(_, h)| h.file == file)
            .map(|(i, _)| i)
            .collect();
        let sites: Vec<(u64, u64)> = indices
            .iter()
            .map(|&i| (hits[i].line as u64, hits[i].column as u64))
            .collect();
        match fetch(&file, &sites) {
            Ok(answers) => {
                for (&i, answer) in indices.iter().zip(answers) {
                    hits[i].goals = Some(answer);
                }
            }
            Err(e) => {
                for &i in &indices {
                    hits[i].goals_error = Some(e.to_string());
                }
            }
        }
    }
}

/// Scan `files` (paths under `project_root`) and return every hit, in file and position order.
pub fn sorry_hits(project_root: &Path, files: &[PathBuf]) -> Result<Vec<SorryHit>> {
    let ws = WorkspaceRoot::new(project_root)?;
    let mut hits = Vec::new();
    for file in files {
        let target = ws.resolve(&file.to_string_lossy())?;
        let rel = relative_display(ws.path(), &target);
        let src =
            std::fs::read_to_string(&target).map_err(|e| tpt_lean_core::Error::from(e).with_path(&target))?;
        hits.extend(hits_in_source(&rel, &src));
    }
    Ok(hits)
}

/// Hits in one source text, labelled with `file`.
pub fn hits_in_source(file: &str, src: &str) -> Vec<SorryHit> {
    let parsed = tpt_lean_syntax::parse(src);
    let mut out = Vec::new();
    for tok in &parsed.tokens {
        if !matches!(tok.kind, TokenKind::Ident | TokenKind::Keyword) {
            continue;
        }
        let text = tok.range.text(src);
        let keyword = match text {
            "sorry" => "sorry",
            "admit" => "admit",
            _ => continue,
        };
        let position = parsed.lines.position(tok.range.start);
        // The nearest declaration that starts at or before the hit.
        let declaration = parsed
            .symbols
            .iter()
            .filter(|s| s.range.start <= tok.range.start)
            .max_by_key(|s| s.range.start)
            .map(|s| s.name.clone());
        out.push(SorryHit {
            file: file.to_string(),
            line: position.line,
            column: position.column + 1,
            keyword,
            declaration,
            goals: None,
            goals_error: None,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_sorry_and_admit_in_declarations() {
        let src = "theorem a : True := by\n  sorry\n\ntheorem b : True := by\n  admit\n";
        let hits = hits_in_source("A.lean", src);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].keyword, "sorry");
        assert_eq!((hits[0].line, hits[0].column), (2, 3));
        assert_eq!(hits[0].declaration.as_deref(), Some("a"));
        assert_eq!(hits[1].keyword, "admit");
        assert_eq!(hits[1].declaration.as_deref(), Some("b"));
    }

    #[test]
    fn ignores_comments_strings_and_longer_names() {
        let src = "-- sorry in a comment\n/- admit in a block -/\n\
                   def s : String := \"sorry\"\ndef sorryLike : Nat := 1\n";
        assert!(hits_in_source("A.lean", src).is_empty());
    }

    #[test]
    fn attach_goals_answers_each_file_once_and_keeps_failures_local() {
        let src = |file: &str| hits_in_source(file, "theorem a : True := by\n  sorry\n");
        let mut hits = src("A.lean");
        hits.extend(src("B.lean"));
        let mut asked = Vec::new();
        attach_goals(&mut hits, |file, sites| {
            asked.push((file.to_string(), sites.to_vec()));
            if file == "B.lean" {
                return Err(tpt_lean_core::Error::invalid("cannot elaborate B.lean"));
            }
            Ok(sites
                .iter()
                .map(|&(line, column)| SiteGoals {
                    line,
                    column,
                    target: tpt_lean_adapter::sidecar::GoalTarget::Tactic,
                    goals: vec!["⊢ True".into()],
                })
                .collect())
        });
        assert_eq!(
            asked,
            vec![
                ("A.lean".to_string(), vec![(2, 3)]),
                ("B.lean".to_string(), vec![(2, 3)]),
            ]
        );
        assert_eq!(hits[0].goals.as_ref().unwrap().goals, vec!["⊢ True"]);
        assert!(hits[0].goals_error.is_none());
        assert!(hits[1].goals.is_none());
        assert!(hits[1].goals_error.as_deref().unwrap().contains("B.lean"));
    }

    #[test]
    fn a_hit_before_any_declaration_has_no_declaration() {
        let hits = hits_in_source("A.lean", "sorry\n");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].declaration, None);
    }
}
