// SPDX-License-Identifier: MIT OR Apache-2.0
//! Lean language-server helpers for the MCP tools: goals and diagnostics at a position, the
//! LSP-to-tool diagnostic shape, and context-pack assembly under a token budget.

use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;
use tpt_lean_adapter::{self as adapter, lsp};
use tpt_lean_syntax::position::LineIndex;

/// Lines of source kept on each side of the position at full budget.
const FULL_RADIUS: usize = 6;
/// Diagnostics included in a context pack before the list is cut.
const MAX_PACK_DIAGNOSTICS: usize = 20;

/// What the language server reported for one document at one position.
#[derive(Debug, Default)]
pub(crate) struct Snapshot {
    /// Plain goals at the position (empty when Lean reports none).
    pub goals: Vec<String>,
    /// Raw LSP diagnostics published for the document.
    pub diagnostics: Vec<Value>,
}

/// UTF-16 column for a 1-based line and 1-based character column.
pub(crate) fn utf16_column(text: &str, line: u32, column: u32) -> Result<u32, String> {
    let index = LineIndex::new(text);
    let offset = index
        .offset_of(line as usize, column as usize)
        .ok_or_else(|| format!("{line}:{column} is outside the file"))?;
    Ok(index.position(offset).column_utf16 as u32)
}

/// Start `lake serve` (or `lean --server`), open the document, and ask for goals at a 1-based
/// position. The server is shut down before this returns.
///
/// Diagnostics are whatever the server has published when the goal request is answered.
pub(crate) fn elaborate(
    root: &Path,
    path: &Path,
    text: &str,
    line: u32,
    column: u32,
    timeout_seconds: u64,
) -> Result<Snapshot, String> {
    let character = utf16_column(text, line, column)?;
    let tc = adapter::discover_toolchain();
    let (program, args) =
        lsp::server_command(root, tc.lake.as_ref(), tc.lean.as_ref()).map_err(|e| e.to_string())?;
    let limit = Duration::from_secs(timeout_seconds.max(1));
    let mut client = lsp::LspClient::start(&program, &args, root, limit).map_err(|e| e.to_string())?;
    let result = (|| -> Result<Snapshot, String> {
        let uri = lsp::open_document(&mut client, path, text).map_err(|e| e.to_string())?;
        let goals =
            lsp::plain_goals(&mut client, &uri, line - 1, character, limit).map_err(|e| e.to_string())?;
        let diagnostics = client.diagnostics_for(&uri);
        Ok(Snapshot { goals, diagnostics })
    })();
    client.shutdown(Duration::from_secs(5));
    result
}

/// Severity name used in tool output (LSP codes 1 to 4).
pub(crate) fn severity_name(code: Option<u64>) -> &'static str {
    match code {
        Some(2) => "warning",
        Some(3) => "info",
        Some(4) => "hint",
        _ => "error",
    }
}

/// An LSP diagnostic in the same shape as the adapter's CLI diagnostics:
/// `{file, line, column, severity, message}` with 1-based line and column.
pub(crate) fn lsp_diagnostic(file: &str, d: &Value) -> Value {
    json!({
        "file": file,
        "line": d["range"]["start"]["line"].as_u64().unwrap_or(0) + 1,
        "column": d["range"]["start"]["character"].as_u64().unwrap_or(0) + 1,
        "end_line": d["range"]["end"]["line"].as_u64().unwrap_or(0) + 1,
        "end_column": d["range"]["end"]["character"].as_u64().unwrap_or(0) + 1,
        "severity": severity_name(d["severity"].as_u64()),
        "message": d["message"].as_str().unwrap_or(""),
    })
}

/// Whether the LSP range of `d` contains the 0-based `(line, character)` position.
pub(crate) fn diagnostic_covers(d: &Value, line0: u64, character: u64) -> bool {
    let start = (
        d["range"]["start"]["line"].as_u64().unwrap_or(0),
        d["range"]["start"]["character"].as_u64().unwrap_or(0),
    );
    let end = (
        d["range"]["end"]["line"].as_u64().unwrap_or(0),
        d["range"]["end"]["character"].as_u64().unwrap_or(0),
    );
    start <= (line0, character) && (line0, character) <= end
}

/// Split a plain goal into hypotheses (lines before `⊢`) and the target.
pub(crate) fn split_goal(goal: &str) -> (Vec<String>, Option<String>) {
    let mut hypotheses = Vec::new();
    let mut target = None;
    for line in goal.lines() {
        if let Some(t) = line.strip_prefix("⊢ ") {
            target = Some(t.to_string());
        } else if !line.trim().is_empty() {
            hypotheses.push(line.to_string());
        }
    }
    (hypotheses, target)
}

/// Rough token count: one token per four characters, rounded up.
pub(crate) fn approx_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

/// Numbered source lines within `radius` of the 1-based `line`.
pub(crate) fn excerpt_around(text: &str, line: usize, radius: usize) -> String {
    let index = LineIndex::new(text);
    let total = index.line_count();
    let from = line.saturating_sub(radius).max(1);
    let to = (line + radius).min(total);
    let mut out = Vec::new();
    for n in from..=to {
        let start = index.line_start(n).unwrap_or(0);
        let end = index.line_start(n + 1).unwrap_or(text.len());
        let content = text[start..end].trim_end_matches(['\r', '\n']);
        out.push(format!("{n:>5} | {content}"));
    }
    out.join("\n")
}

/// Everything a context pack is built from.
pub(crate) struct PackInput<'a> {
    pub file: &'a str,
    pub line: u32,
    pub column: u32,
    pub text: &'a str,
    pub goal: Option<String>,
    pub hypotheses: Vec<String>,
    /// Diagnostics in tool shape (see [`lsp_diagnostic`]).
    pub diagnostics: Vec<Value>,
    /// Candidate symbols, most relevant first.
    pub symbols: Vec<Value>,
    pub max_tokens: usize,
}

/// Assemble the context pack of specification section 11.4 within `max_tokens`.
///
/// When the estimate is over budget, parts are dropped in this order: symbols, then the
/// source window (6, 3, 1, then 0 lines), then hypotheses, then the goal text. `truncated`
/// is set whenever anything was dropped.
pub(crate) fn assemble_context(input: PackInput<'_>) -> Value {
    let PackInput {
        file,
        line,
        column,
        text,
        goal: first_goal,
        hypotheses: first_hypotheses,
        diagnostics: first_diagnostics,
        symbols: first_symbols,
        max_tokens,
    } = input;
    let mut truncated = false;
    let mut symbols = first_symbols;
    let mut hypotheses = first_hypotheses;
    let mut goal = first_goal;
    let mut diagnostics = first_diagnostics;
    if diagnostics.len() > MAX_PACK_DIAGNOSTICS {
        diagnostics.truncate(MAX_PACK_DIAGNOSTICS);
        truncated = true;
    }
    let mut radius = FULL_RADIUS;
    loop {
        let excerpt = excerpt_around(text, line as usize, radius);
        let fields = PackFields {
            file,
            line,
            column,
            max_tokens,
            goal: goal.as_deref(),
            hypotheses: &hypotheses,
            diagnostics: &diagnostics,
            symbols: &symbols,
            excerpt: &excerpt,
            truncated,
        };
        let pack = render_pack(&fields);
        if approx_tokens(&pack.to_string()) <= max_tokens {
            return pack;
        }
        truncated = true;
        if symbols.pop().is_some() {
            continue;
        }
        if radius > 0 {
            radius = match radius {
                r if r > 3 => 3,
                r if r > 1 => 1,
                _ => 0,
            };
            continue;
        }
        if hypotheses.pop().is_some() {
            continue;
        }
        if let Some(g) = goal.as_mut().filter(|g| g.chars().count() > 1) {
            let keep = g.chars().count() / 2;
            *g = g.chars().take(keep).collect();
            continue;
        }
        return pack;
    }
}

/// The parts of a pack at one level of truncation.
struct PackFields<'a> {
    file: &'a str,
    line: u32,
    column: u32,
    max_tokens: usize,
    goal: Option<&'a str>,
    hypotheses: &'a [String],
    diagnostics: &'a [Value],
    symbols: &'a [Value],
    excerpt: &'a str,
    truncated: bool,
}

fn render_pack(f: &PackFields<'_>) -> Value {
    let mut pack = json!({
        "file": f.file,
        "position": { "line": f.line, "column": f.column },
        "goal": f.goal,
        "hypotheses": f.hypotheses,
        "diagnostics": f.diagnostics,
        "relevant_symbols": f.symbols,
        "source_excerpt": f.excerpt,
        "max_tokens": f.max_tokens,
        "truncated": f.truncated,
    });
    let estimate = approx_tokens(&pack.to_string());
    pack["estimated_tokens"] = json!(estimate);
    pack
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diag(sl: u64, sc: u64, el: u64, ec: u64, sev: u64, msg: &str) -> Value {
        json!({
            "range": {"start": {"line": sl, "character": sc}, "end": {"line": el, "character": ec}},
            "severity": sev,
            "message": msg
        })
    }

    #[test]
    fn goal_is_split_into_hypotheses_and_target() {
        let (h, t) = split_goal("a b : Nat\nh : a = b\n⊢ b = a");
        assert_eq!(h, vec!["a b : Nat".to_string(), "h : a = b".to_string()]);
        assert_eq!(t.as_deref(), Some("b = a"));
    }

    #[test]
    fn position_coverage_is_inclusive() {
        let d = diag(2, 4, 2, 9, 1, "x");
        assert!(diagnostic_covers(&d, 2, 4));
        assert!(diagnostic_covers(&d, 2, 9));
        assert!(!diagnostic_covers(&d, 2, 10));
        assert!(!diagnostic_covers(&d, 1, 5));
    }

    #[test]
    fn lsp_diagnostic_uses_one_based_positions_and_names() {
        let v = lsp_diagnostic("A.lean", &diag(0, 0, 0, 3, 2, "warn"));
        assert_eq!(v["line"], 1);
        assert_eq!(v["column"], 1);
        assert_eq!(v["severity"], "warning");
        assert_eq!(v["message"], "warn");
    }

    #[test]
    fn utf16_column_counts_surrogate_pairs() {
        // "𝔸" is one character but two UTF-16 units.
        let text = "𝔸 = b\n";
        assert_eq!(utf16_column(text, 1, 1).unwrap(), 0);
        assert_eq!(utf16_column(text, 1, 2).unwrap(), 2);
        assert!(utf16_column(text, 9, 1).is_err());
    }

    #[test]
    fn excerpt_is_numbered_and_bounded() {
        let text = "one\ntwo\nthree\nfour\nfive\n";
        let ex = excerpt_around(text, 3, 1);
        assert_eq!(ex, "    2 | two\n    3 | three\n    4 | four");
    }

    fn sample_input(text: &str, max_tokens: usize) -> PackInput<'_> {
        PackInput {
            file: "A.lean",
            line: 2,
            column: 3,
            text,
            goal: Some("⊢ a + b = b + a".into()),
            hypotheses: vec!["a : Nat".into(), "b : Nat".into()],
            diagnostics: vec![],
            symbols: vec![json!({"name": "Nat.add_comm"}), json!({"name": "Nat.add_assoc"})],
            max_tokens,
        }
    }

    #[test]
    fn context_pack_has_spec_shape_when_it_fits() {
        let text = "theorem t (a b : Nat) : a + b = b + a := by\n  skip\n";
        let pack = assemble_context(sample_input(text, 6000));
        assert_eq!(pack["file"], "A.lean");
        assert_eq!(pack["position"], json!({"line": 2, "column": 3}));
        assert_eq!(pack["goal"], "⊢ a + b = b + a");
        assert_eq!(pack["hypotheses"].as_array().unwrap().len(), 2);
        assert_eq!(pack["relevant_symbols"].as_array().unwrap().len(), 2);
        assert!(pack["source_excerpt"].as_str().unwrap().contains("skip"));
        assert_eq!(pack["truncated"], false);
    }

    #[test]
    fn context_pack_respects_budget_and_marks_truncation() {
        let text = "theorem t := by\n  skip\n";
        let pack = assemble_context(sample_input(text, 40));
        assert_eq!(pack["truncated"], true);
        assert!(
            approx_tokens(&pack.to_string()) <= 40
                || pack["goal"].as_str().is_none_or(|g| g.chars().count() <= 1)
        );
        // Symbols are the first thing dropped.
        assert_eq!(pack["relevant_symbols"].as_array().unwrap().len(), 0);
    }
}
