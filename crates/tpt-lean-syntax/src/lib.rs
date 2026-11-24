// SPDX-License-Identifier: MIT OR Apache-2.0
//! Lossless, lightweight syntax layer for Lean 4 files.
//!
//! This is not an elaborator. It provides tokenization, a shallow concrete syntax tree,
//! comment and whitespace preservation, indentation facts, import extraction, declaration
//! boundaries, tactic-block detection, and symbol extraction. Semantic facts (types, goals,
//! proof validity) must come from Lean through `tpt-lean-adapter`.

pub mod cst;
pub mod lexer;
pub mod position;

use cst::{SyntaxElement, SyntaxKind, SyntaxNode, DECL_KEYWORDS};
use lexer::{lex, TextRange, Token, TokenKind};
use position::LineIndex;

/// Result of parsing one source file.
#[derive(Debug, Clone)]
pub struct Parse {
    pub source_len: usize,
    pub tokens: Vec<Token>,
    pub root: SyntaxNode,
    pub lines: LineIndex,
    pub imports: Vec<Import>,
    pub symbols: Vec<Symbol>,
    pub tactic_blocks: Vec<TacticBlock>,
    /// Places where the parse is not trustworthy. A formatter must refuse to write when non-empty.
    pub uncertain: Vec<Uncertain>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    pub module: String,
    pub range: TextRange,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    /// Fully qualified name when the namespace is known, otherwise the declared name.
    pub name: String,
    /// Declared name as written.
    pub short_name: Option<String>,
    /// Declaration keyword (`theorem`, `def`, ...).
    pub kind: String,
    pub namespace: String,
    pub range: TextRange,
    pub line: usize,
    pub docstring: Option<String>,
    pub is_private: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TacticBlock {
    /// Line of the `by` keyword.
    pub start_line: usize,
    /// Last line whose content is indented deeper than the line containing `by`.
    pub end_line: usize,
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Uncertain {
    pub range: TextRange,
    pub reason: &'static str,
}

/// Parse a Lean source file.
pub fn parse(src: &str) -> Parse {
    let tokens = lex(src);
    let lines = LineIndex::new(src);
    let root = cst::build(src, &tokens);
    let imports = extract_imports(src, &tokens, &lines);
    let symbols = extract_symbols(src, &tokens, &lines, &root);
    let tactic_blocks = find_tactic_blocks(src, &tokens, &lines);
    let uncertain = find_uncertain(src, &tokens, &lines);
    Parse {
        source_len: src.len(),
        tokens,
        root,
        lines,
        imports,
        symbols,
        tactic_blocks,
        uncertain,
    }
}

impl Parse {
    /// Rebuild the source text from the tree (must equal the input).
    pub fn to_text(&self, src: &str) -> String {
        let mut out = String::with_capacity(src.len());
        self.root.write_text(src, &mut out);
        out
    }

    /// True when the parse has no uncertain regions.
    pub fn is_certain(&self) -> bool {
        self.uncertain.is_empty()
    }
}

/// Indentation (number of leading spaces) of a 1-based line.
pub fn indentation_of_line(src: &str, lines: &LineIndex, line: usize) -> usize {
    let Some(start) = lines.line_start(line) else {
        return 0;
    };
    src[start..].chars().take_while(|c| *c == ' ').count()
}

fn significant(tok: &Token) -> bool {
    !matches!(
        tok.kind,
        TokenKind::Whitespace | TokenKind::Newline | TokenKind::LineComment | TokenKind::BlockComment
    )
}

fn extract_imports(src: &str, tokens: &[Token], lines: &LineIndex) -> Vec<Import> {
    let sig: Vec<&Token> = tokens.iter().filter(|t| significant(t)).collect();
    let mut out = Vec::new();
    for (i, t) in sig.iter().enumerate() {
        if t.kind == TokenKind::Keyword
            && t.range.text(src) == "import"
            && lines.column_of(t.range.start) == 0
        {
            if let Some(m) = sig.get(i + 1) {
                if m.kind == TokenKind::Ident {
                    out.push(Import {
                        module: m.range.text(src).to_string(),
                        range: TextRange::new(t.range.start, m.range.end),
                        line: lines.line_of(t.range.start),
                    });
                }
            }
        }
    }
    out
}

fn extract_symbols(src: &str, tokens: &[Token], lines: &LineIndex, root: &SyntaxNode) -> Vec<Symbol> {
    let _ = tokens;
    let mut namespaces: Vec<String> = Vec::new();
    let mut sections: Vec<Option<String>> = Vec::new();
    let mut symbols = Vec::new();

    for item in &root.children {
        let SyntaxElement::Node(node) = item else { continue };
        let toks: Vec<Token> = node.tokens().into_iter().filter(significant).collect();
        let Some(first) = toks.first() else { continue };
        let first_text = first.range.text(src);

        match node.kind {
            SyntaxKind::Scope => match first_text {
                "namespace" => {
                    let name = toks
                        .get(1)
                        .map(|t| t.range.text(src).to_string())
                        .unwrap_or_default();
                    namespaces.push(name.clone());
                    sections.push(Some(name));
                }
                "section" => {
                    let name = toks
                        .get(1)
                        .filter(|t| t.kind == TokenKind::Ident)
                        .map(|t| t.range.text(src).to_string());
                    sections.push(name);
                }
                "end" => {
                    if let Some(Some(name)) = sections.pop() {
                        if namespaces.last().map(|n| n == &name).unwrap_or(false) {
                            namespaces.pop();
                        }
                    }
                }
                _ => {}
            },
            SyntaxKind::Decl => {
                let Some(kw_idx) = toks
                    .iter()
                    .position(|t| t.kind == TokenKind::Keyword && DECL_KEYWORDS.contains(&t.range.text(src)))
                else {
                    continue;
                };
                let kind = toks[kw_idx].range.text(src).to_string();
                let is_private = toks[..kw_idx].iter().any(|t| t.range.text(src) == "private");
                let short = toks
                    .get(kw_idx + 1)
                    .filter(|t| t.kind == TokenKind::Ident)
                    .map(|t| t.range.text(src).to_string());
                let docstring = toks[..kw_idx]
                    .iter()
                    .find(|t| t.kind == TokenKind::DocComment)
                    .map(|t| clean_doc(t.range.text(src)));
                let namespace = namespaces.join(".");
                let name = match (&short, namespace.is_empty()) {
                    (Some(s), true) => s.clone(),
                    (Some(s), false) => format!("{namespace}.{s}"),
                    (None, _) => kind.clone(),
                };
                let decl_start = toks[kw_idx].range.start;
                symbols.push(Symbol {
                    name,
                    short_name: short,
                    kind,
                    namespace,
                    range: TextRange::new(decl_start, node.range.end),
                    line: lines.line_of(decl_start),
                    docstring,
                    is_private,
                });
            }
            _ => {}
        }
    }
    symbols
}

fn clean_doc(raw: &str) -> String {
    let inner = raw.trim_start_matches("/--").trim_end_matches("-/");
    inner.trim().to_string()
}

fn find_tactic_blocks(src: &str, tokens: &[Token], lines: &LineIndex) -> Vec<TacticBlock> {
    let sig: Vec<&Token> = tokens.iter().filter(|t| significant(t)).collect();
    let mut out = Vec::new();
    for (i, t) in sig.iter().enumerate() {
        if !(t.kind == TokenKind::Keyword && t.range.text(src) == "by") {
            continue;
        }
        let by_line = lines.line_of(t.range.start);
        let base = indentation_of_line(src, lines, by_line);
        let mut end_line = by_line;
        let mut last_offset = t.range.end;
        for later in sig.iter().skip(i + 1) {
            let line = lines.line_of(later.range.start);
            if line == by_line {
                last_offset = later.range.end;
                continue;
            }
            if lines.column_of(later.range.start) > base {
                end_line = line;
                last_offset = later.range.end;
            } else {
                break;
            }
        }
        out.push(TacticBlock {
            start_line: by_line,
            end_line,
            range: TextRange::new(t.range.start, last_offset),
        });
    }
    out
}

fn find_uncertain(src: &str, tokens: &[Token], lines: &LineIndex) -> Vec<Uncertain> {
    let mut out = Vec::new();
    for t in tokens {
        if t.unterminated {
            out.push(Uncertain {
                range: t.range,
                reason: "unterminated token",
            });
        }
        if t.kind == TokenKind::Unknown {
            out.push(Uncertain {
                range: t.range,
                reason: "unrecognised character",
            });
        }
    }
    // Tabs in leading indentation make column-based formatting ambiguous.
    for line in 1..=lines.line_count() {
        let Some(start) = lines.line_start(line) else {
            continue;
        };
        let indent: String = src[start..]
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect();
        if let Some(pos) = indent.find('\t') {
            let at = start + pos;
            out.push(Uncertain {
                range: TextRange::new(at, at + 1),
                reason: "tab in indentation",
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "import Mathlib.Data.Nat\nimport Std\n\nnamespace Foo\n\n/-- Adds commutatively. -/\ntheorem add_comm' (a b : Nat) : a + b = b + a := by\n  simp [Nat.add_comm]\n  omega\n\nprivate def helper := 1\n\nend Foo\n\ntheorem top : True := trivial\n";

    #[test]
    fn roundtrip_is_exact() {
        let p = parse(SAMPLE);
        assert_eq!(p.to_text(SAMPLE), SAMPLE);
    }

    #[test]
    fn extracts_imports() {
        let p = parse(SAMPLE);
        let mods: Vec<&str> = p.imports.iter().map(|i| i.module.as_str()).collect();
        assert_eq!(mods, vec!["Mathlib.Data.Nat", "Std"]);
        assert_eq!(p.imports[0].line, 1);
    }

    #[test]
    fn extracts_symbols_with_namespace_and_docs() {
        let p = parse(SAMPLE);
        let names: Vec<&str> = p.symbols.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["Foo.add_comm'", "Foo.helper", "top"]);
        assert_eq!(p.symbols[0].docstring.as_deref(), Some("Adds commutatively."));
        assert_eq!(p.symbols[0].kind, "theorem");
        assert!(p.symbols[1].is_private);
    }

    #[test]
    fn finds_tactic_block_extent() {
        let p = parse(SAMPLE);
        assert_eq!(p.tactic_blocks.len(), 1);
        assert_eq!(p.tactic_blocks[0].start_line, 7);
        assert_eq!(p.tactic_blocks[0].end_line, 9);
    }

    #[test]
    fn tabs_and_unterminated_are_uncertain() {
        assert!(!parse("theorem x := by\n\tsimp\n").is_certain());
        assert!(!parse("/- never closed").is_certain());
        assert!(parse(SAMPLE).is_certain());
    }

    #[test]
    fn symbol_names_survive_unicode() {
        let p = parse("def αβ := 1\n");
        assert_eq!(p.symbols[0].name, "αβ");
    }
}

#[cfg(test)]
mod fuzz_like {
    use super::*;

    /// Deterministic pseudo-random inputs: the parser must never panic and must round-trip exactly.
    #[test]
    fn random_inputs_roundtrip_without_panics() {
        const ALPHABET: &[&str] = &[
            "theorem",
            "def",
            "by",
            "import",
            "namespace",
            "end",
            " ",
            "\n",
            "\t",
            "/-",
            "-/",
            "/--",
            "--",
            "\"",
            "'",
            "(",
            ")",
            ":=",
            "→",
            "α",
            "«",
            "»",
            ".",
            "x",
            "1",
            "@[",
            "]",
            "\\",
            "\r\n",
            "λ",
            "😀",
            "\u{0}",
        ];
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        for _ in 0..2000 {
            let mut s = String::new();
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let len = (seed >> 33) % 40;
            for _ in 0..len {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                s.push_str(ALPHABET[((seed >> 33) as usize) % ALPHABET.len()]);
            }
            let p = parse(&s);
            assert_eq!(p.to_text(&s), s, "round-trip failed for {s:?}");
        }
    }
}
