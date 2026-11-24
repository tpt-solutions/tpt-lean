// SPDX-License-Identifier: MIT OR Apache-2.0
//! Pure (non-wasm) logic for tokenizing, formatting and viewing the CST of Lean source.
//!
//! Every function returns a JSON string with an `ok` flag. Offsets are UTF-8 byte offsets
//! into the input, as produced by `tpt-lean-syntax`.

use serde::Serialize;
use tpt_lean_fmt::FmtError;
use tpt_lean_syntax::cst::{SyntaxElement, SyntaxKind, SyntaxNode};
use tpt_lean_syntax::lexer::{lex, Token, TokenKind};
use tpt_lean_syntax::position::LineIndex;

/// Upper bound on source size accepted by the browser API (8 MiB).
pub const MAX_SOURCE_BYTES: usize = 8 * 1024 * 1024;

/// Name of a token kind as exposed in JSON.
pub fn token_kind_name(kind: TokenKind) -> &'static str {
    match kind {
        TokenKind::Ident => "ident",
        TokenKind::Keyword => "keyword",
        TokenKind::Number => "number",
        TokenKind::String => "string",
        TokenKind::Char => "char",
        TokenKind::Whitespace => "whitespace",
        TokenKind::Newline => "newline",
        TokenKind::LineComment => "lineComment",
        TokenKind::BlockComment => "blockComment",
        TokenKind::DocComment => "docComment",
        TokenKind::ModuleDocComment => "moduleDocComment",
        TokenKind::Symbol => "symbol",
        TokenKind::Unknown => "unknown",
    }
}

/// Name of a CST node kind as exposed in JSON.
pub fn syntax_kind_name(kind: SyntaxKind) -> &'static str {
    match kind {
        SyntaxKind::File => "file",
        SyntaxKind::Import => "import",
        SyntaxKind::Scope => "scope",
        SyntaxKind::Decl => "decl",
        SyntaxKind::Command => "command",
        SyntaxKind::Trivia => "trivia",
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TokenOut<'a> {
    kind: &'static str,
    start: usize,
    end: usize,
    line: usize,
    text: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TokenizeOk<'a> {
    ok: bool,
    tokens: Vec<TokenOut<'a>>,
}

#[derive(Serialize)]
struct ErrorOut {
    ok: bool,
    error: String,
}

#[derive(Serialize)]
struct Uncertain {
    start: usize,
    reason: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FormatOk {
    ok: bool,
    output: String,
    changed: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FormatRefused {
    ok: bool,
    error: String,
    uncertain: Vec<Uncertain>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum TreeOut {
    Node {
        kind: &'static str,
        start: usize,
        end: usize,
        children: Vec<TreeOut>,
    },
    Token {
        kind: &'static str,
        start: usize,
        end: usize,
        text: String,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CstOk {
    ok: bool,
    certain: bool,
    tree: TreeOut,
}

fn error_json(msg: impl Into<String>) -> String {
    to_json(&ErrorOut {
        ok: false,
        error: msg.into(),
    })
}

fn to_json<T: Serialize>(value: &T) -> String {
    // Serialization of these plain structs cannot fail; fall back to a fixed error envelope.
    serde_json::to_string(value)
        .unwrap_or_else(|_| r#"{"ok":false,"error":"serialization failed"}"#.to_string())
}

fn check_size(src: &str) -> Result<(), String> {
    if src.len() > MAX_SOURCE_BYTES {
        Err(format!(
            "source is {} bytes; the limit is {MAX_SOURCE_BYTES}",
            src.len()
        ))
    } else {
        Ok(())
    }
}

/// Lossless token list: `{"ok":true,"tokens":[{kind,start,end,line,text}]}`.
pub fn tokenize_json(src: &str) -> String {
    if let Err(e) = check_size(src) {
        return error_json(e);
    }
    let lines = LineIndex::new(src);
    let tokens: Vec<Token> = lex(src);
    let tokens = tokens
        .iter()
        .map(|t| TokenOut {
            kind: token_kind_name(t.kind),
            start: t.range.start,
            end: t.range.end,
            line: lines.line_of(t.range.start),
            text: t.range.text(src),
        })
        .collect();
    to_json(&TokenizeOk { ok: true, tokens })
}

/// Format Lean source. On success `{"ok":true,"output","changed"}`; when the parse is
/// uncertain the formatter refuses: `{"ok":false,"error","uncertain":[{start,reason}]}`.
pub fn format_json(src: &str) -> String {
    if let Err(e) = check_size(src) {
        return error_json(e);
    }
    match tpt_lean_fmt::format_text(src) {
        Ok(f) => to_json(&FormatOk {
            ok: true,
            output: f.output,
            changed: f.changed,
        }),
        Err(err) => {
            let FmtError::Uncertain(list) = &err;
            let uncertain = list
                .iter()
                .map(|(start, reason)| Uncertain {
                    start: *start,
                    reason,
                })
                .collect();
            to_json(&FormatRefused {
                ok: false,
                error: err.to_string(),
                uncertain,
            })
        }
    }
}

fn tree_of(src: &str, node: &SyntaxNode) -> TreeOut {
    TreeOut::Node {
        kind: syntax_kind_name(node.kind),
        start: node.range.start,
        end: node.range.end,
        children: node
            .children
            .iter()
            .map(|child| match child {
                SyntaxElement::Node(n) => tree_of(src, n),
                SyntaxElement::Token(t) => TreeOut::Token {
                    kind: token_kind_name(t.kind),
                    start: t.range.start,
                    end: t.range.end,
                    text: t.range.text(src).to_string(),
                },
            })
            .collect(),
    }
}

/// Shallow concrete syntax tree: `{"ok":true,"certain":bool,"tree":{type,kind,start,end,children}}`.
/// Leaves have `"type":"token"` with `kind` set to the token kind and `text` set to the source text.
pub fn cst_json(src: &str) -> String {
    if let Err(e) = check_size(src) {
        return error_json(e);
    }
    let parsed = tpt_lean_syntax::parse(src);
    to_json(&CstOk {
        ok: true,
        certain: parsed.is_certain(),
        tree: tree_of(src, &parsed.root),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(s: &str) -> serde_json::Value {
        serde_json::from_str(s).expect("valid json")
    }

    #[test]
    fn tokenize_reports_kinds_and_lines() {
        let v = json(&tokenize_json("theorem t : True := trivial\n-- c\n"));
        assert_eq!(v["ok"], true);
        let toks = v["tokens"].as_array().unwrap();
        assert_eq!(toks[0]["kind"], "keyword");
        assert_eq!(toks[0]["text"], "theorem");
        assert_eq!(toks[0]["line"], 1);
        let comment = toks.iter().find(|t| t["kind"] == "lineComment").unwrap();
        assert_eq!(comment["line"], 2);
    }

    #[test]
    fn tokenize_is_lossless_and_offsets_are_bytes() {
        let src = "def café := 1\n";
        let v = json(&tokenize_json(src));
        let joined: String = v["tokens"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["text"].as_str().unwrap())
            .collect();
        assert_eq!(joined, src);
        let last = v["tokens"].as_array().unwrap().last().unwrap();
        assert_eq!(last["end"].as_u64().unwrap() as usize, src.len());
    }

    #[test]
    fn format_success_and_idempotence() {
        let v = json(&format_json("def  x   :=  1   \n\n\n"));
        assert_eq!(v["ok"], true);
        assert_eq!(v["changed"], true);
        let out = v["output"].as_str().unwrap().to_string();
        let again = json(&format_json(&out));
        assert_eq!(again["changed"], false);
        assert_eq!(again["output"], out);
    }

    #[test]
    fn oversized_input_is_rejected() {
        let big = "a".repeat(MAX_SOURCE_BYTES + 1);
        for out in [tokenize_json(&big), format_json(&big), cst_json(&big)] {
            let v = json(&out);
            assert_eq!(v["ok"], false);
            assert!(v["error"].as_str().unwrap().contains("limit"));
        }
    }

    #[test]
    fn cst_tree_is_lossless_and_has_decl() {
        let src = "import Mathlib\n\ntheorem t : True := trivial\n";
        let v = json(&cst_json(src));
        assert_eq!(v["ok"], true);
        assert_eq!(v["tree"]["type"], "node");
        assert_eq!(v["tree"]["kind"], "file");
        assert_eq!(v["tree"]["start"], 0);
        assert_eq!(v["tree"]["end"].as_u64().unwrap() as usize, src.len());
        let kinds: Vec<&str> = v["tree"]["children"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|c| c["kind"].as_str())
            .collect();
        assert!(kinds.contains(&"import"));
        assert!(kinds.contains(&"decl"));
    }
}
