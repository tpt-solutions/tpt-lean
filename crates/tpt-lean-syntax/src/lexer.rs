// SPDX-License-Identifier: MIT OR Apache-2.0
//! Lossless tokenizer for Lean 4 source.
//!
//! Every byte of the input belongs to exactly one token, so concatenating token texts
//! reproduces the input exactly. Whitespace and comments are tokens, not discarded.

/// Byte range `[start, end)` into the source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TextRange {
    pub start: usize,
    pub end: usize,
}

impl TextRange {
    pub fn new(start: usize, end: usize) -> Self {
        debug_assert!(start <= end);
        Self { start, end }
    }

    pub fn len(&self) -> usize {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    pub fn text<'a>(&self, src: &'a str) -> &'a str {
        &src[self.start..self.end]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// Identifier, possibly dotted (`Nat.add_comm`), may contain guillemets `«»`.
    Ident,
    /// Reserved keyword (see [`KEYWORDS`]).
    Keyword,
    Number,
    String,
    Char,
    Whitespace,
    Newline,
    /// `-- ...` up to end of line.
    LineComment,
    /// `/- ... -/`, nesting allowed. Not a doc comment.
    BlockComment,
    /// `/-- ... -/` documentation comment.
    DocComment,
    /// `/-! ... -/` module doc comment.
    ModuleDocComment,
    /// Operator or punctuation, ASCII or Unicode.
    Symbol,
    /// Anything the lexer could not classify. Marks the parse as uncertain.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub range: TextRange,
    /// True when the token is unterminated (string or comment hit end of input).
    pub unterminated: bool,
}

/// Reserved words for declarations, commands and term/tactic syntax the parser cares about.
pub const KEYWORDS: &[&str] = &[
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
    "mutual",
    "extends",
    "where",
    "by",
    "fun",
    "have",
    "show",
    "let",
    "match",
    "with",
    "at",
    "from",
    "if",
    "then",
    "else",
    "do",
    "return",
    "set_option",
    "attribute",
    "deriving",
    "initialize",
    "macro",
    "syntax",
    "notation",
    "elab",
    "termination_by",
    "decreasing_by",
    "calc",
    "suffices",
    "obtain",
    "exact",
    "intro",
    "simp",
    "rw",
    "case",
    "using",
    "in",
    "for",
    "unless",
    "mut",
    "try",
    "catch",
    "finally",
];

/// Multi-character ASCII operators, longest first so that the lexer matches greedily.
const ASCII_OPS: &[&str] = &[
    "<;>", "<|>", "...", "::", ":=", "=>", "->", "<-", "<=", ">=", "==", "!=", "&&", "||", "++", "**", "<|",
    "|>", "..", "%[", "⟨", "@[", "∘", "<&>", "|>.", "^^^",
];

/// Lex `src` into a contiguous, lossless token stream.
pub fn lex(src: &str) -> Vec<Token> {
    let mut lx = Lexer {
        src,
        pos: 0,
        out: Vec::new(),
    };
    while lx.pos < src.len() {
        lx.next_token();
    }
    lx.out
}

struct Lexer<'a> {
    src: &'a str,
    pos: usize,
    out: Vec<Token>,
}

fn is_ident_start(c: char) -> bool {
    // λ, Π and Σ are Lean symbols, not identifier letters, even though they are alphabetic.
    (c.is_alphabetic() && !matches!(c, 'λ' | 'Π' | 'Σ')) || c == '_' || c == '«' || is_lean_letterlike(c)
}

fn is_ident_rest(c: char) -> bool {
    c.is_alphanumeric()
        || c == '_'
        || c == '\''
        || c == '!'
        || c == '?'
        || c == '»'
        || c == '«'
        || is_subscript(c)
        || is_lean_letterlike(c)
}

/// Lean's letter-like Unicode blocks: Greek (except λ, Π, Σ), Letterlike symbols, subscripts.
fn is_lean_letterlike(c: char) -> bool {
    matches!(c, 'α'..='ω' | 'Α'..='Ω' | 'ℕ' | 'ℤ' | 'ℚ' | 'ℝ' | 'ℂ' | 'ℍ' | '𝕜')
        && c != 'λ'
        && c != 'Π'
        && c != 'Σ'
        || is_subscript(c)
}

fn is_subscript(c: char) -> bool {
    matches!(c, '₀'..='₉' | 'ₐ'..='ₜ' | 'ᵢ'..='ᵪ' | 'ⱼ')
}

impl<'a> Lexer<'a> {
    fn rest(&self) -> &'a str {
        &self.src[self.pos..]
    }

    fn push(&mut self, kind: TokenKind, end: usize, unterminated: bool) {
        self.out.push(Token {
            kind,
            range: TextRange::new(self.pos, end),
            unterminated,
        });
        self.pos = end;
    }

    fn next_token(&mut self) {
        let rest = self.rest();
        let c = rest.chars().next().expect("pos < len");
        let clen = c.len_utf8();

        if c == '\n' {
            return self.push(TokenKind::Newline, self.pos + 1, false);
        }
        if c == '\r' && rest.as_bytes().get(1) == Some(&b'\n') {
            return self.push(TokenKind::Newline, self.pos + 2, false);
        }
        if c.is_whitespace() {
            let len: usize = rest
                .chars()
                .take_while(|ch| ch.is_whitespace() && *ch != '\n' && *ch != '\r')
                .map(char::len_utf8)
                .sum();
            let len = len.max(clen);
            return self.push(TokenKind::Whitespace, self.pos + len, false);
        }
        if rest.starts_with("--") {
            let end = rest.find('\n').map(|i| self.pos + i).unwrap_or(self.src.len());
            return self.push(TokenKind::LineComment, end, false);
        }
        if rest.starts_with("/-") {
            return self.block_comment();
        }
        if c == '"' {
            return self.string_literal();
        }
        if c == '\'' {
            if let Some(end) = self.char_literal_end() {
                return self.push(TokenKind::Char, end, false);
            }
        }
        if c.is_ascii_digit() {
            let len: usize = rest
                .chars()
                .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '.')
                .map(char::len_utf8)
                .sum();
            return self.push(TokenKind::Number, self.pos + len, false);
        }
        if is_ident_start(c) {
            return self.identifier_or_keyword();
        }
        for op in ASCII_OPS {
            if rest.starts_with(op) {
                return self.push(TokenKind::Symbol, self.pos + op.len(), false);
            }
        }
        if c.is_ascii_punctuation() || is_unicode_symbol(c) {
            return self.push(TokenKind::Symbol, self.pos + clen, false);
        }
        self.push(TokenKind::Unknown, self.pos + clen, false);
    }

    fn block_comment(&mut self) {
        let rest = self.rest();
        let kind = if rest.starts_with("/--") && !rest.starts_with("/--/") {
            TokenKind::DocComment
        } else if rest.starts_with("/-!") {
            TokenKind::ModuleDocComment
        } else {
            TokenKind::BlockComment
        };
        // Nested comments: `/-` opens, `-/` closes.
        let bytes = rest.as_bytes();
        let mut depth = 0usize;
        let mut i = 0usize;
        while i < bytes.len() {
            if bytes[i..].starts_with(b"/-") {
                depth += 1;
                i += 2;
            } else if bytes[i..].starts_with(b"-/") {
                depth -= 1;
                i += 2;
                if depth == 0 {
                    return self.push(kind, self.pos + i, false);
                }
            } else {
                i += 1;
            }
        }
        self.push(kind, self.src.len(), true)
    }

    fn string_literal(&mut self) {
        let rest = self.rest();
        let mut escaped = false;
        for (i, ch) in rest.char_indices().skip(1) {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                return self.push(TokenKind::String, self.pos + i + 1, false);
            }
        }
        self.push(TokenKind::String, self.src.len(), true)
    }

    /// If a well-formed character literal starts here, return its end offset.
    fn char_literal_end(&self) -> Option<usize> {
        let rest = self.rest();
        let mut it = rest.char_indices();
        it.next()?; // opening quote
        let (_, first) = it.next()?;
        if first == '\\' {
            let (_, _) = it.next()?;
            // Allow \u{...} style escapes loosely: find the closing quote.
            let (close, q) = it.find(|(_, ch)| *ch == '\'')?;
            let _ = q;
            return Some(self.pos + close + 1);
        }
        let (close, q) = it.next()?;
        (q == '\'').then_some(self.pos + close + 1)
    }

    fn identifier_or_keyword(&mut self) {
        let rest = self.rest();
        let mut end = 0usize;
        let mut segment_start = true;
        let mut chars = rest.char_indices().peekable();
        while let Some((i, ch)) = chars.next() {
            if segment_start {
                if !is_ident_start(ch) {
                    break;
                }
                segment_start = false;
                end = i + ch.len_utf8();
                if ch == '«' {
                    // Guillemet-quoted segment: consume through closing »
                    let close = rest[i..].find('»').map(|k| i + k + '»'.len_utf8());
                    match close {
                        Some(c) => {
                            end = c;
                            // Skip the chars we just consumed.
                            while let Some(&(j, _)) = chars.peek() {
                                if j < c {
                                    chars.next();
                                } else {
                                    break;
                                }
                            }
                        }
                        None => break,
                    }
                    segment_start = false;
                }
                continue;
            }
            if is_ident_rest(ch) {
                end = i + ch.len_utf8();
            } else if ch == '.' {
                // A dot continues the identifier only if an identifier segment follows.
                match chars.peek() {
                    Some(&(_, next)) if is_ident_start(next) => {
                        end = i + 1;
                        segment_start = true;
                    }
                    _ => break,
                }
            } else {
                break;
            }
        }
        let text = &rest[..end];
        let kind = if !text.contains('.') && KEYWORDS.contains(&text) {
            TokenKind::Keyword
        } else {
            TokenKind::Ident
        };
        self.push(kind, self.pos + end, false);
    }
}

/// Unicode symbols that Lean uses as operators or delimiters.
fn is_unicode_symbol(c: char) -> bool {
    matches!(
        c,
        '→' | '←'
            | '↔'
            | '⇒'
            | '⇔'
            | '∀'
            | '∃'
            | 'λ'
            | 'Π'
            | 'Σ'
            | '∧'
            | '∨'
            | '¬'
            | '≠'
            | '≤'
            | '≥'
            | '∈'
            | '∉'
            | '⊆'
            | '⊂'
            | '∪'
            | '∩'
            | '×'
            | '∘'
            | '·'
            | '•'
            | '⟨'
            | '⟩'
            | '⟦'
            | '⟧'
            | '∑'
            | '∏'
            | '⊢'
            | '⊤'
            | '⊥'
            | '∣'
            | '∅'
            | '≈'
            | '≃'
            | '↑'
            | '∞'
            | '√'
            | '∫'
            | '⁻'
            | '¹'
            | '²'
            | '³'
            | '≡'
            | '⊗'
            | '⊕'
            | '▸'
            | '‹'
            | '›'
            | '⦃'
            | '⦄'
            | '⦇'
            | '⦈'
            | '∗'
            | '⋆'
            | '⊔'
            | '⊓'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(src: &str) {
        let toks = lex(src);
        let mut rebuilt = String::new();
        let mut expect = 0;
        for t in &toks {
            assert_eq!(t.range.start, expect, "tokens must be contiguous");
            rebuilt.push_str(t.range.text(src));
            expect = t.range.end;
        }
        assert_eq!(rebuilt, src);
        assert_eq!(expect, src.len());
    }

    #[test]
    fn lossless_on_samples() {
        roundtrip("theorem foo : 1 + 1 = 2 := by\n  rfl -- done\n");
        roundtrip("/- outer /- inner -/ still -/ def x := 1");
        roundtrip("/-- doc -/\nlemma a : True := trivial\n");
        roundtrip("\"unterminated");
        roundtrip("/- open comment");
        roundtrip("def «weird name» := 'x' + '\\n' ∀ x, x ≤ y → ⟨a, b⟩");
        roundtrip("\r\nfoo\r\n");
        roundtrip("αβγ ℕ x₁ h' ∑ ∅ \u{1F600}");
    }

    #[test]
    fn dotted_identifier_is_one_token() {
        let src = "Nat.add_comm";
        let toks = lex(src);
        assert_eq!(toks.len(), 1);
        assert_eq!(toks[0].kind, TokenKind::Ident);
    }

    #[test]
    fn keywords_recognised() {
        let toks = lex("theorem x");
        assert_eq!(toks[0].kind, TokenKind::Keyword);
        assert_eq!(toks[2].kind, TokenKind::Ident);
    }

    #[test]
    fn nested_comment_closes_only_at_depth_zero() {
        let src = "/- a /- b -/ c -/x";
        let toks = lex(src);
        assert_eq!(toks[0].kind, TokenKind::BlockComment);
        assert_eq!(toks[0].range.text(src), "/- a /- b -/ c -/");
        assert!(!toks[0].unterminated);
    }

    #[test]
    fn unterminated_flags_set() {
        assert!(lex("\"abc").last().unwrap().unterminated);
        assert!(lex("/- abc").last().unwrap().unterminated);
    }

    #[test]
    fn doc_comment_kinds() {
        assert_eq!(lex("/-- hi -/")[0].kind, TokenKind::DocComment);
        assert_eq!(lex("/-! mod -/")[0].kind, TokenKind::ModuleDocComment);
    }
}
