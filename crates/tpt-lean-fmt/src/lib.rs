// SPDX-License-Identifier: MIT OR Apache-2.0
//! Conservative, syntax-preserving formatter for Lean 4.
//!
//! Rules (all idempotent; comment and string contents are never touched):
//! - strip trailing whitespace on lines, and blank out whitespace-only lines
//! - ensure exactly one final newline (for non-empty files)
//! - collapse runs of spaces between tokens on one line to one space, and remove spaces
//!   before `,` and `)`/`]`/`}`
//! - single space around `:=`, `=>`, `->`, `<-`, `→`, `←`, `↔`
//! - sort and dedupe the leading contiguous block of `import` lines
//! - indent the body of a `by` block that ends its line to at least two columns deeper
//!   than the line containing `by`
//!
//! The formatter refuses to write when the syntax layer reports uncertain regions.

use tpt_lean_syntax::lexer::{TextRange, Token, TokenKind};

/// Errors from formatting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FmtError {
    /// The parse contains uncertain regions; the file is left untouched.
    Uncertain(Vec<(usize, &'static str)>),
}

impl std::fmt::Display for FmtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FmtError::Uncertain(list) => {
                write!(f, "refusing to format: {} uncertain region(s)", list.len())?;
                for (offset, reason) in list.iter().take(5) {
                    write!(f, "; byte {offset}: {reason}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for FmtError {}

/// Result of a successful format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Formatted {
    pub output: String,
    pub changed: bool,
}

/// A contiguous changed region in a line-based diff (1-based, inclusive for the old side).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: usize,
    pub old_lines: Vec<String>,
    pub new_lines: Vec<String>,
}

/// Format `src`. Returns the same text when nothing needs to change.
pub fn format_text(src: &str) -> Result<Formatted, FmtError> {
    let parse = tpt_lean_syntax::parse(src);
    if !parse.is_certain() {
        return Err(FmtError::Uncertain(
            parse
                .uncertain
                .iter()
                .map(|u| (u.range.start, u.reason))
                .collect(),
        ));
    }

    let step1 = rewrite_tokens(src, &parse.tokens);
    let step2 = sort_imports(&step1);
    let step3 = indent_tactic_blocks(&step2);
    let output = ensure_final_newline(&step3);
    Ok(Formatted {
        changed: output != src,
        output,
    })
}

/// Compute the minimal changed hunk between two texts (prefix/suffix trimming).
/// Returns `None` when they are identical.
pub fn diff(old: &str, new: &str) -> Option<Hunk> {
    if old == new {
        return None;
    }
    let a: Vec<&str> = old.split_inclusive('\n').collect();
    let b: Vec<&str> = new.split_inclusive('\n').collect();
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let max_suffix = (a.len() - prefix).min(b.len() - prefix);
    let suffix = (0..max_suffix)
        .take_while(|&i| a[a.len() - 1 - i] == b[b.len() - 1 - i])
        .count();
    Some(Hunk {
        old_start: prefix + 1,
        old_lines: a[prefix..a.len() - suffix]
            .iter()
            .map(|s| s.trim_end_matches('\n').to_string())
            .collect(),
        new_lines: b[prefix..b.len() - suffix]
            .iter()
            .map(|s| s.trim_end_matches('\n').to_string())
            .collect(),
    })
}

/// Render a [`Hunk`] as a unified-style text block.
pub fn render_hunk(path: &str, hunk: &Hunk) -> String {
    let mut out = format!(
        "--- {path}\n+++ {path}\n@@ -{},{} +{},{} @@\n",
        hunk.old_start,
        hunk.old_lines.len(),
        hunk.old_start,
        hunk.new_lines.len()
    );
    for l in &hunk.old_lines {
        out.push_str(&format!("-{l}\n"));
    }
    for l in &hunk.new_lines {
        out.push_str(&format!("+{l}\n"));
    }
    out
}

/// Token-level rewrites: trailing whitespace, spacing around operators, and comma spacing.
fn rewrite_tokens(src: &str, tokens: &[Token]) -> String {
    const SPACED_OPS: &[&str] = &[":=", "=>", "->", "<-", "→", "←", "↔"];
    let is_ws = |t: &Token| t.kind == TokenKind::Whitespace;
    let is_eol = |t: Option<&Token>| match t {
        None => true,
        Some(t) => matches!(t.kind, TokenKind::Newline),
    };
    let text = |t: &Token| t.range.text(src);

    let mut out = String::with_capacity(src.len());
    for (i, tok) in tokens.iter().enumerate() {
        let prev = i.checked_sub(1).and_then(|j| tokens.get(j));
        let next = tokens.get(i + 1);

        if is_ws(tok) {
            // Trailing whitespace before a newline or EOF is removed.
            if is_eol(next) {
                continue;
            }
            // Whitespace-only leading indentation is kept (handled by indentation rules).
            if prev.is_none_or(|p| p.kind == TokenKind::Newline) {
                out.push_str(text(tok));
                continue;
            }
            // Collapse runs of spaces between tokens to one space.
            let before_comma =
                next.is_some_and(|n| n.kind == TokenKind::Symbol && matches!(text(n), "," | ")" | "]" | "}"));
            if before_comma {
                continue;
            }
            let next_is_comment = next.is_some_and(|n| {
                matches!(
                    n.kind,
                    TokenKind::LineComment | TokenKind::BlockComment | TokenKind::DocComment
                )
            });
            if next_is_comment {
                // Keep comment alignment exactly as written.
                out.push_str(text(tok));
                continue;
            }
            out.push(' ');
            continue;
        }

        // Spacing around spaced operators: ensure a space on each side when on the same line.
        if tok.kind == TokenKind::Symbol && SPACED_OPS.contains(&text(tok)) {
            let prev_is_text =
                prev.is_some_and(|p| !matches!(p.kind, TokenKind::Newline | TokenKind::Whitespace));
            if prev_is_text && !out.ends_with(' ') && !out.ends_with('\n') && !out.is_empty() {
                out.push(' ');
            }
            out.push_str(text(tok));
            let next_is_ws_or_eol =
                next.is_none_or(|n| matches!(n.kind, TokenKind::Whitespace | TokenKind::Newline));
            if !next_is_ws_or_eol {
                out.push(' ');
            }
            continue;
        }

        out.push_str(text(tok));
    }
    out
}

/// Sort and dedupe the leading block of import lines. Blank lines or other code end the block.
fn sort_imports(src: &str) -> String {
    let lines: Vec<&str> = src.split_inclusive('\n').collect();
    let mut i = 0;
    // Skip leading blank lines and file-level comments.
    while i < lines.len() && (lines[i].trim().is_empty() || lines[i].trim_start().starts_with("--")) {
        i += 1;
    }
    let start = i;
    while i < lines.len() && lines[i].starts_with("import ") {
        i += 1;
    }
    let end = i;
    if end - start < 2 {
        return src.to_string();
    }
    let mut block: Vec<String> = lines[start..end]
        .iter()
        .map(|l| l.trim_end_matches(['\r', '\n']).to_string())
        .collect();
    let eol = if lines[start].ends_with("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    block.sort();
    block.dedup();
    let mut out = String::with_capacity(src.len());
    for l in &lines[..start] {
        out.push_str(l);
    }
    for l in &block {
        out.push_str(l);
        out.push_str(eol);
    }
    for l in &lines[end..] {
        out.push_str(l);
    }
    out
}

/// When a `by` ends its line, shift the contiguous run of body lines that follows it so the
/// body starts at least two columns deeper than the line containing `by`. The run stops at a
/// blank line, a column-0 command, a comment line, or a line inside a multi-line token.
fn indent_tactic_blocks(src: &str) -> String {
    let parse = tpt_lean_syntax::parse(src);
    let lines: Vec<&str> = src.split_inclusive('\n').collect();
    let inside_token = |line: usize| {
        let Some(start) = parse.lines.line_start(line) else {
            return false;
        };
        parse
            .tokens
            .iter()
            .any(|t| t.range.start < start && t.range.end > start)
    };
    let mut bump: std::collections::BTreeMap<usize, usize> = std::collections::BTreeMap::new();
    for block in parse
        .tactic_blocks
        .iter()
        .filter(|b| b.kind == tpt_lean_syntax::TacticKind::By)
    {
        let by_line = block.start_line;
        if !line_tail_is_by(src, &parse, by_line) {
            continue;
        }
        let base = tpt_lean_syntax::indentation_of_line(src, &parse.lines, by_line);
        let mut run: Vec<usize> = Vec::new();
        for line in (by_line + 1)..=lines.len() {
            let raw = lines[line - 1].trim_end_matches(['\r', '\n']);
            let content = raw.trim_start();
            if content.is_empty()
                || content.starts_with("--")
                || content.starts_with("/-")
                || inside_token(line)
            {
                break;
            }
            let indent = raw.len() - content.len();
            if indent == 0 && starts_command(content) {
                break;
            }
            run.push(line);
        }
        let Some(min_indent) = run
            .iter()
            .map(|&l| tpt_lean_syntax::indentation_of_line(src, &parse.lines, l))
            .min()
        else {
            continue;
        };
        let target = base + 2;
        if min_indent < target {
            let delta = target - min_indent;
            for &l in &run {
                let current = tpt_lean_syntax::indentation_of_line(src, &parse.lines, l);
                bump.insert(l, current + delta);
            }
        }
    }
    if bump.is_empty() {
        return src.to_string();
    }
    let mut out = String::with_capacity(src.len() + bump.len() * 2);
    for (idx, l) in lines.iter().enumerate() {
        match bump.get(&(idx + 1)) {
            Some(width) => {
                let existing = l.chars().take_while(|c| *c == ' ').count();
                out.push_str(&" ".repeat(*width));
                out.push_str(&l[existing..]);
            }
            None => out.push_str(l),
        }
    }
    out
}

/// True when a line (already left-trimmed) begins with a top-level command keyword.
fn starts_command(content: &str) -> bool {
    const STARTERS: &[&str] = &[
        "theorem",
        "lemma",
        "def",
        "instance",
        "structure",
        "inductive",
        "class",
        "abbrev",
        "example",
        "axiom",
        "opaque",
        "namespace",
        "section",
        "end",
        "open",
        "import",
        "variable",
        "universe",
        "private",
        "protected",
        "noncomputable",
        "partial",
        "unsafe",
        "set_option",
        "attribute",
        "@[",
        "/--",
        "/-!",
    ];
    let first = content
        .split(|c: char| c.is_whitespace() || c == '(' || c == '[')
        .next()
        .unwrap_or("");
    STARTERS.contains(&first) || content.starts_with("@[") || content.starts_with("/--")
}

/// True when the last significant token on `line` is the `by` keyword.
fn line_tail_is_by(src: &str, parse: &tpt_lean_syntax::Parse, line: usize) -> bool {
    let Some(start) = parse.lines.line_start(line) else {
        return false;
    };
    let end = parse.lines.line_start(line + 1).unwrap_or(src.len());
    parse
        .tokens
        .iter()
        .rfind(|t| {
            t.range.start >= start
                && t.range.end <= end
                && !matches!(
                    t.kind,
                    TokenKind::Whitespace | TokenKind::Newline | TokenKind::LineComment
                )
        })
        .is_some_and(|t| t.kind == TokenKind::Keyword && t.range.text(src) == "by")
}

fn ensure_final_newline(src: &str) -> String {
    if src.is_empty() {
        return String::new();
    }
    let trimmed = src.trim_end_matches(['\n', '\r']);
    if trimmed.len() == src.len() {
        return src.to_string();
    }
    // Keep the file's own line ending style for the single final newline.
    let eol = if src.contains("\r\n") { "\r\n" } else { "\n" };
    // Collapse extra trailing blank lines to exactly one line break.
    format!("{trimmed}{eol}")
}

/// Byte range helper kept public for callers that want to map diagnostics to edits.
pub fn range_of(token: &Token) -> TextRange {
    token.range
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trailing_spaces_and_final_newline() {
        let out = format_text("def x := 1   \n\n\n").unwrap();
        assert_eq!(out.output, "def x := 1\n");
        assert!(out.changed);
    }

    #[test]
    fn comma_and_operator_spacing() {
        let out = format_text("def f := g (a ,b)  ->  c\n").unwrap();
        assert_eq!(out.output, "def f := g (a,b) -> c\n");
    }

    #[test]
    fn imports_sorted_and_deduped() {
        let src = "import Std\nimport Mathlib.Data\nimport Std\n\ntheorem t : True := trivial\n";
        let out = format_text(src).unwrap();
        assert_eq!(
            out.output,
            "import Mathlib.Data\nimport Std\n\ntheorem t : True := trivial\n"
        );
    }

    #[test]
    fn comments_and_strings_untouched() {
        let src = "-- trailing   \ndef s := \"a  ,  b   \"   \n";
        let out = format_text(src).unwrap();
        assert!(out.output.contains("-- trailing   \n"));
        assert!(out.output.contains("\"a  ,  b   \""));
    }

    #[test]
    fn tactic_body_is_indented_under_by() {
        let src = "theorem t : True := by\ntrivial\n";
        let out = format_text(src).unwrap();
        assert_eq!(out.output, "theorem t : True := by\n  trivial\n");
    }

    #[test]
    fn refuses_uncertain_input() {
        assert!(format_text("theorem t := by\n\tsimp\n").is_err());
    }

    #[test]
    fn idempotent() {
        let samples = [
            "import Std\nimport Alpha\n\ndef x := (1 ,2)  \n",
            "theorem t : True := by\ntrivial  \n\n",
            "",
            "\n\n",
        ];
        for s in samples {
            let once = format_text(s).unwrap().output;
            let twice = format_text(&once).unwrap().output;
            assert_eq!(once, twice, "not idempotent for {s:?}");
        }
    }

    #[test]
    fn diff_reports_minimal_hunk() {
        let h = diff("a\nb\nc\n", "a\nB\nc\n").unwrap();
        assert_eq!(h.old_start, 2);
        assert_eq!(h.old_lines, vec!["b".to_string()]);
        assert_eq!(h.new_lines, vec!["B".to_string()]);
        assert!(diff("same\n", "same\n").is_none());
    }
}
