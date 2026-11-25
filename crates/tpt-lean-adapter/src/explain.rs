// SPDX-License-Identifier: MIT OR Apache-2.0
//! Explaining one position in a Lean file: the error Lean reports there, the goal, and lemma
//! candidates from the project index. The language-server round trip lives here so that the CLI
//! and the JSON-RPC server share it.

use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};
use tpt_lean_core::dirs::ProjectDirs;
use tpt_lean_core::{Error, Result};
use tpt_lean_index::Index;
use tpt_lean_syntax::position::LineIndex;

use crate::lsp::{self, LspClient};

/// An error Lean reports at a position. Lines and columns are 1-based.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExplainError {
    pub message: String,
    pub file: String,
    pub line: u64,
    pub column: u64,
}

/// What the language server said about one position.
#[derive(Debug, Clone, PartialEq)]
pub struct PositionSnapshot {
    /// 0-based line of the position.
    pub line0: u64,
    /// UTF-16 column of the position, which is how the language server counts columns.
    pub utf16: u64,
    /// Plain goals at the position. The first is the one usually shown.
    pub goals: Vec<String>,
    /// Every diagnostic the language server reported for the document.
    pub diagnostics: Vec<Value>,
}

/// Ask the Lean language server about one position. `line` and `column` are 1-based, with the
/// column counted in Unicode scalar values. `label` names the file in error messages.
pub fn snapshot_at(
    root: &Path,
    file: &Path,
    label: &str,
    text: &str,
    line: usize,
    column: usize,
    timeout: Duration,
) -> Result<PositionSnapshot> {
    let index = LineIndex::new(text);
    let offset = index
        .offset_of(line, column)
        .ok_or_else(|| Error::invalid(format!("{label}:{line}:{column} is outside the file")))?;
    let utf16 = index.position(offset).column_utf16 as u64;
    let tc = crate::discover_toolchain();
    let (program, server_args) = lsp::server_command(root, tc.lake.as_ref(), tc.lean.as_ref())?;
    let mut client = LspClient::start(&program, &server_args, root, timeout)?;
    let uri = lsp::open_document(&mut client, file, text)?;
    let goals = lsp::plain_goals(&mut client, &uri, (line - 1) as u32, utf16 as u32, timeout);
    let diagnostics = await_diagnostics(&mut client, &uri, timeout.min(DIAGNOSTICS_WAIT));
    client.shutdown(Duration::from_secs(5));
    Ok(PositionSnapshot {
        line0: (line - 1) as u64,
        utf16,
        goals: goals?,
        diagnostics,
    })
}

/// Longest time to wait for the server to publish diagnostics for a document.
const DIAGNOSTICS_WAIT: Duration = Duration::from_secs(30);

/// Diagnostics for `uri` once the server has finished elaborating it. Lean publishes an empty
/// list when a document opens, and reports processing progress as it works, so a publication
/// alone is not enough: this polls until progress reports no work left and a publication has
/// arrived. If `wait` runs out first, whatever has been published so far is returned.
fn await_diagnostics(client: &mut LspClient, uri: &str, wait: Duration) -> Vec<Value> {
    let deadline = Instant::now() + wait;
    loop {
        let done = client.file_progress(uri) == Some(0);
        if done {
            if let Some(published) = client.published_diagnostics(uri) {
                return published;
            }
        }
        if Instant::now() >= deadline {
            return client.diagnostics_for(uri);
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// The first error that covers the position, if any. In LSP, severity 1 is an error.
pub fn error_at(diagnostics: &[Value], file: &str, line0: u64, utf16: u64) -> Option<ExplainError> {
    diagnostics
        .iter()
        .find(|d| {
            let s = &d["range"]["start"];
            let e = &d["range"]["end"];
            let start = (
                s["line"].as_u64().unwrap_or(0),
                s["character"].as_u64().unwrap_or(0),
            );
            let end = (
                e["line"].as_u64().unwrap_or(0),
                e["character"].as_u64().unwrap_or(0),
            );
            start <= (line0, utf16) && (line0, utf16) <= end && d["severity"].as_u64().unwrap_or(1) == 1
        })
        .map(|d| ExplainError {
            message: d["message"].as_str().unwrap_or("").to_string(),
            file: file.to_string(),
            line: d["range"]["start"]["line"].as_u64().unwrap_or(0) + 1,
            column: d["range"]["start"]["character"].as_u64().unwrap_or(0) + 1,
        })
}

/// Split a plain goal into hypotheses (the lines before `⊢`) and the target.
pub fn split_goal(goal: &str) -> (Vec<String>, Option<String>) {
    let mut hyps = Vec::new();
    let mut target = None;
    for line in goal.lines() {
        if let Some(t) = line.strip_prefix("⊢ ") {
            target = Some(t.to_string());
        } else if !line.trim().is_empty() {
            hyps.push(line.to_string());
        }
    }
    (hyps, target)
}

/// Lemma candidates for a goal from the project index, as JSON objects. Empty when the project
/// has no index yet.
pub fn lemma_candidates(root: &Path, goal: &str, limit: usize) -> Vec<Value> {
    let dirs = ProjectDirs::new(root);
    let Ok(index) = Index::open(&dirs.index_db()) else {
        return Vec::new();
    };
    crate::suggest_lemmas(&index, goal, limit)
        .unwrap_or_default()
        .into_iter()
        .map(
            |h| json!({ "name": h.name, "kind": h.kind, "source": h.file, "line": h.line, "score": h.score }),
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_hypotheses_from_target() {
        let (hyps, target) = split_goal("a b : Nat\nh : a = b\n⊢ b = a");
        assert_eq!(hyps, vec!["a b : Nat".to_string(), "h : a = b".to_string()]);
        assert_eq!(target.as_deref(), Some("b = a"));
        assert_eq!(split_goal(""), (Vec::new(), None));
    }

    #[test]
    fn error_at_picks_the_covering_error() {
        let diags = vec![
            json!({"severity": 2, "message": "warn", "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 9}}}),
            json!({"severity": 1, "message": "boom", "range": {"start": {"line": 2, "character": 4}, "end": {"line": 2, "character": 8}}}),
        ];
        let e = error_at(&diags, "A.lean", 2, 5).expect("covered");
        assert_eq!((e.message.as_str(), e.line, e.column), ("boom", 3, 5));
        assert!(
            error_at(&diags, "A.lean", 0, 1).is_none(),
            "warnings are not errors"
        );
        assert!(error_at(&diags, "A.lean", 9, 0).is_none());
    }
}
