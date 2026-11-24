// SPDX-License-Identifier: MIT OR Apache-2.0
//! Lossless concrete syntax tree for Lean files, built on top of the token stream.
//!
//! The tree groups tokens into top-level *items* (declarations, imports, namespaces, ...).
//! It is deliberately shallow: it is not an elaborator. Full semantics come from Lean.

use crate::lexer::{lex, TextRange, Token, TokenKind};
use crate::position::LineIndex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntaxKind {
    /// Whole file.
    File,
    /// `import A.B` command.
    Import,
    /// `namespace`, `section` or `end` command.
    Scope,
    /// Declaration (`theorem`, `def`, ...), including its leading doc comment and attributes.
    Decl,
    /// Anything else at top level (`open`, `variable`, `set_option`, ...).
    Command,
    /// Leading trivia that does not belong to any item (blank lines, file-level comments).
    Trivia,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyntaxElement {
    Node(SyntaxNode),
    Token(Token),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxNode {
    pub kind: SyntaxKind,
    pub range: TextRange,
    pub children: Vec<SyntaxElement>,
}

impl SyntaxNode {
    /// Rebuild the exact source text covered by this node.
    pub fn write_text(&self, src: &str, out: &mut String) {
        for child in &self.children {
            match child {
                SyntaxElement::Node(n) => n.write_text(src, out),
                SyntaxElement::Token(t) => out.push_str(t.range.text(src)),
            }
        }
    }

    /// Iterate over all tokens in source order.
    pub fn tokens(&self) -> Vec<Token> {
        let mut out = Vec::new();
        collect_tokens(self, &mut out);
        out
    }
}

fn collect_tokens(node: &SyntaxNode, out: &mut Vec<Token>) {
    for child in &node.children {
        match child {
            SyntaxElement::Node(n) => collect_tokens(n, out),
            SyntaxElement::Token(t) => out.push(*t),
        }
    }
}

/// Keywords that start a declaration.
pub const DECL_KEYWORDS: &[&str] = &[
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
];

/// Modifiers that may precede a declaration keyword.
const MODIFIERS: &[&str] = &[
    "private",
    "protected",
    "noncomputable",
    "partial",
    "unsafe",
    "nonrec",
];

/// Top-level command keywords that do not declare a name.
const COMMAND_KEYWORDS: &[&str] = &[
    "open",
    "variable",
    "universe",
    "set_option",
    "attribute",
    "macro",
    "syntax",
    "notation",
    "elab",
    "mutual",
    "deriving",
    "initialize",
    "end",
];

/// Build the item tree for `src`. Every byte of `src` appears exactly once in the result.
pub fn build(src: &str, tokens: &[Token]) -> SyntaxNode {
    let lines = LineIndex::new(src);
    let mut items: Vec<SyntaxNode> = Vec::new();
    let mut current: Vec<SyntaxElement> = Vec::new();
    let mut current_kind: Option<SyntaxKind> = None;
    // True while the open item holds only a prefix (doc comment / attribute / modifiers).
    let mut prefix_only = false;
    let mut current_start: usize = 0;

    let flush = |items: &mut Vec<SyntaxNode>,
                 current: &mut Vec<SyntaxElement>,
                 kind: &mut Option<SyntaxKind>,
                 start: usize| {
        if current.is_empty() {
            return;
        }
        let end = end_of(current);
        items.push(SyntaxNode {
            kind: kind.unwrap_or(SyntaxKind::Trivia),
            range: TextRange::new(start, end),
            children: std::mem::take(current),
        });
        *kind = None;
    };

    for tok in tokens {
        let is_trivia = matches!(
            tok.kind,
            TokenKind::Whitespace | TokenKind::Newline | TokenKind::LineComment | TokenKind::BlockComment
        );
        let at_line_start = lines.column_of(tok.range.start) == 0;
        let text = tok.range.text(src);

        if at_line_start && !is_trivia {
            let is_decl = tok.kind == TokenKind::Keyword && DECL_KEYWORDS.contains(&text);
            let is_modifier = tok.kind == TokenKind::Keyword && MODIFIERS.contains(&text);
            let is_prefix = tok.kind == TokenKind::DocComment || text.starts_with("@[");
            let is_command = tok.kind == TokenKind::Keyword
                && (text == "import"
                    || text == "namespace"
                    || text == "section"
                    || COMMAND_KEYWORDS.contains(&text));

            let starts_new = if is_decl || is_modifier || is_prefix || is_command {
                // Do not split when the open item is only a prefix that this token completes.
                !(prefix_only && (is_decl || is_modifier))
            } else {
                // A column-0 identifier or symbol that is not a command: attach to the current item.
                false
            };
            if starts_new {
                flush(&mut items, &mut current, &mut current_kind, current_start);
                current_start = tok.range.start;
                prefix_only = is_prefix || is_modifier;
                current_kind = Some(if is_decl || is_modifier || is_prefix {
                    SyntaxKind::Decl
                } else if text == "import" {
                    SyntaxKind::Import
                } else if text == "namespace" || text == "section" || text == "end" {
                    SyntaxKind::Scope
                } else {
                    SyntaxKind::Command
                });
            } else if is_decl {
                // Prefix-only item completed by its declaration keyword.
                current_kind = Some(SyntaxKind::Decl);
                prefix_only = false;
            }
        }

        if current.is_empty() {
            current_start = tok.range.start;
            if current_kind.is_none() && !is_trivia {
                current_kind = Some(SyntaxKind::Trivia);
            }
        }
        if !is_trivia && !matches!(tok.kind, TokenKind::DocComment | TokenKind::ModuleDocComment) {
            // Once a non-prefix token appears, the item is no longer prefix-only.
            if !(tok.kind == TokenKind::Keyword
                && (MODIFIERS.contains(&text) || DECL_KEYWORDS.contains(&text)))
                && !text.starts_with("@[")
            {
                prefix_only = false;
            }
        }
        current.push(SyntaxElement::Token(*tok));
    }
    flush(&mut items, &mut current, &mut current_kind, current_start);

    let children = items.into_iter().map(SyntaxElement::Node).collect::<Vec<_>>();
    SyntaxNode {
        kind: SyntaxKind::File,
        range: TextRange::new(0, src.len()),
        children,
    }
}

fn end_of(elements: &[SyntaxElement]) -> usize {
    match elements.last() {
        Some(SyntaxElement::Token(t)) => t.range.end,
        Some(SyntaxElement::Node(n)) => n.range.end,
        None => 0,
    }
}

/// Convenience: lex and build in one step.
pub fn parse_tree(src: &str) -> SyntaxNode {
    build(src, &lex(src))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_is_lossless() {
        let src = "import Mathlib.Data\n\n/-- doc -/\n@[simp]\ntheorem foo : True := trivial\n\nnamespace A\nend A\n";
        let tree = parse_tree(src);
        let mut out = String::new();
        tree.write_text(src, &mut out);
        assert_eq!(out, src);
    }

    #[test]
    fn items_split_at_column_zero_commands() {
        let src = "theorem a : True := trivial\ntheorem b : True := trivial\n";
        let tree = parse_tree(src);
        let decls = tree
            .children
            .iter()
            .filter(|c| matches!(c, SyntaxElement::Node(n) if n.kind == SyntaxKind::Decl))
            .count();
        assert_eq!(decls, 2);
    }

    #[test]
    fn doc_comment_stays_with_declaration() {
        let src = "/-- hello -/\nlemma x : True := trivial\n";
        let tree = parse_tree(src);
        let decl = tree
            .children
            .iter()
            .find_map(|c| match c {
                SyntaxElement::Node(n) if n.kind == SyntaxKind::Decl => Some(n),
                _ => None,
            })
            .expect("decl node");
        assert!(decl.tokens().iter().any(|t| t.kind == TokenKind::DocComment));
    }
}
