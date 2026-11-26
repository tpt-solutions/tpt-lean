// SPDX-License-Identifier: MIT OR Apache-2.0
//! Finds `sorry` and `admit` in Lean sources, per declaration.
//!
//! The scan works on lexer tokens, so a word inside a comment or a string is never reported. A
//! hit is attributed to the nearest declaration above it, as the syntax layer finds them. This is
//! a textual report: it does not elaborate the file, so it cannot see a `sorry` that a macro
//! introduces, and it does not show goals. Lean remains the authority on whether a proof is
//! complete.

use std::path::{Path, PathBuf};

use serde::Serialize;
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
    fn a_hit_before_any_declaration_has_no_declaration() {
        let hits = hits_in_source("A.lean", "sorry\n");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].declaration, None);
    }
}
