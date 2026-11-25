// SPDX-License-Identifier: MIT OR Apache-2.0
//! Context packs for AI agents: a compact bundle around one position in a Lean file. Lean supplies
//! the goal and diagnostics, the file supplies a snippet and nearby declarations, and the index
//! supplies lemma candidates. The pack is trimmed to a token budget, and the snippet is never
//! dropped.

use std::path::Path;
use std::time::Duration;

use serde::Serialize;
use tpt_lean_core::dirs::ProjectDirs;
use tpt_lean_core::Result;
use tpt_lean_syntax::position::LineIndex;

use crate::explain::{snapshot_at, split_goal};

#[derive(Debug, Clone, Serialize)]
pub struct PackPosition {
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct PackDiagnostic {
    pub severity: String,
    pub line: u64,
    pub column: u64,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PackSymbol {
    pub name: String,
    pub kind: String,
    pub line: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct PackLemma {
    pub name: String,
    pub source: String,
    pub line: i64,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContextPack {
    pub file: String,
    pub position: PackPosition,
    pub snippet: String,
    pub goal: Option<String>,
    pub hypotheses: Vec<String>,
    pub diagnostics: Vec<PackDiagnostic>,
    pub imports: Vec<String>,
    pub nearby_definitions: Vec<PackSymbol>,
    pub relevant_lemmas: Vec<PackLemma>,
    pub constraints: Vec<String>,
    pub truncated: bool,
    pub approx_tokens: usize,
}

/// What to build a pack for. `label` is the file name as the caller gave it.
pub struct PackRequest<'a> {
    pub root: &'a Path,
    pub path: &'a Path,
    pub label: &'a str,
    pub text: &'a str,
    /// 1-based line and column.
    pub line: usize,
    pub column: usize,
    pub max_tokens: usize,
    /// Time allowed for the language server round trip.
    pub timeout: Duration,
    /// The configured command timeout, reported in the pack's constraints.
    pub command_timeout_seconds: u64,
}

/// Rough token estimate: about four characters per token.
pub fn approx_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

/// Build the pack for one position, trimming it to `max_tokens` by estimate.
pub fn build_pack(req: &PackRequest) -> Result<ContextPack> {
    let snap = snapshot_at(
        req.root,
        req.path,
        req.label,
        req.text,
        req.line,
        req.column,
        req.timeout,
    )?;
    let goal = snap.goals.first().cloned();
    let (hypotheses, target) = match &goal {
        Some(g) => split_goal(g),
        None => (Vec::new(), None),
    };

    let index = LineIndex::new(req.text);
    let parse = tpt_lean_syntax::parse(req.text);

    // Source: a window of lines around the position.
    let total = index.line_count();
    let from = req.line.saturating_sub(6).max(1);
    let to = (req.line + 6).min(total);
    let mut snippet_lines = Vec::new();
    for n in from..=to {
        let start = index.line_start(n).unwrap_or(0);
        let end = index.line_start(n + 1).unwrap_or(req.text.len());
        let content = req.text[start..end].trim_end_matches(['\r', '\n']);
        snippet_lines.push(format!("{n:>5} | {content}"));
    }
    let snippet = snippet_lines.join("\n");

    let diagnostics: Vec<PackDiagnostic> = snap
        .diagnostics
        .iter()
        .map(|d| PackDiagnostic {
            severity: match d["severity"].as_u64().unwrap_or(1) {
                1 => "error",
                2 => "warning",
                3 => "info",
                _ => "hint",
            }
            .to_string(),
            line: d["range"]["start"]["line"].as_u64().unwrap_or(0) + 1,
            column: d["range"]["start"]["character"].as_u64().unwrap_or(0) + 1,
            message: d["message"].as_str().unwrap_or("").to_string(),
        })
        .collect();

    // Nearby definitions: the closest declarations above the position in this file.
    let mut nearby: Vec<PackSymbol> = parse
        .symbols
        .iter()
        .filter(|s| s.line < req.line)
        .map(|s| PackSymbol {
            name: s.name.clone(),
            kind: s.kind.clone(),
            line: s.line,
        })
        .collect();
    let keep_from = nearby.len().saturating_sub(5);
    nearby.drain(..keep_from);

    // Relevant lemmas: index search on the goal target, when an index exists.
    let mut lemmas = Vec::new();
    let mut constraints = vec![format!(
        "security.workspace_only=true, command_timeout={}s",
        req.command_timeout_seconds
    )];
    let dirs = ProjectDirs::new(req.root);
    if dirs.index_db().exists() {
        if let (Some(t), Ok(idx)) = (&target, tpt_lean_index::Index::open(&dirs.index_db())) {
            let opts = tpt_lean_search::SearchOptions {
                limit: 5,
                ..Default::default()
            };
            let query: String = t
                .chars()
                .map(|c| if c.is_alphanumeric() || c == '_' { c } else { ' ' })
                .collect();
            if let Ok(hits) = tpt_lean_search::search(&idx, query.trim(), &opts) {
                lemmas = hits
                    .into_iter()
                    .map(|h| PackLemma {
                        name: h.name,
                        source: h.file,
                        line: h.line,
                        score: (h.score * 1000.0).round() / 1000.0,
                    })
                    .collect();
            }
        }
    } else {
        constraints.push("no index: run the index scan command to get lemma candidates".into());
    }

    let imports: Vec<String> = parse.imports.iter().map(|i| i.module.clone()).collect();

    // Budget: drop the least important parts until the estimate fits. The snippet is kept whole.
    let mut pack = ContextPack {
        file: req.label.to_string(),
        position: PackPosition {
            line: req.line,
            column: req.column,
        },
        snippet,
        goal,
        hypotheses,
        diagnostics,
        imports,
        nearby_definitions: nearby,
        relevant_lemmas: lemmas,
        constraints,
        truncated: false,
        approx_tokens: 0,
    };
    let estimate = |p: &ContextPack| serde_json::to_string(p).map(|s| approx_tokens(&s)).unwrap_or(0);
    let mut truncated = false;
    while estimate(&pack) > req.max_tokens {
        truncated = true;
        if pack.relevant_lemmas.pop().is_none()
            && (pack.nearby_definitions.is_empty() || {
                pack.nearby_definitions.remove(0);
                false
            })
        {
            break;
        }
    }
    pack.truncated = truncated;
    pack.approx_tokens = estimate(&pack);
    Ok(pack)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_estimate_grows_with_length() {
        assert_eq!(approx_tokens(""), 0);
        assert!(approx_tokens("abcdefgh") < approx_tokens("abcdefghijklmnop"));
    }
}
