// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) TPT Solutions

//! A small `.gitignore` matcher used by the file scan.
//!
//! Supported syntax: blank lines and `#` comments, `!` negation, a trailing `/`
//! for directory-only patterns, a leading or embedded `/` to anchor a pattern to
//! the directory holding its `.gitignore`, and the glob characters `*`, `?`, `**`
//! and `[...]` character classes. Rules from nested `.gitignore` files apply to
//! their own directory and below. The last matching rule wins, as in git.
//! Like git, an ignored directory is never entered, so a negation cannot
//! re-include a file inside it.

use std::path::Path;

/// One parsed line of a `.gitignore` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    /// Directory (relative to the scan root, `/`-separated, empty for the root)
    /// containing the `.gitignore` that declared this rule.
    base: String,
    pattern: String,
    negated: bool,
    dir_only: bool,
    anchored: bool,
}

/// Parse the text of a `.gitignore` file declared in directory `base`.
pub fn parse(text: &str, base: &str) -> Vec<Rule> {
    let mut rules = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches(['\r', ' ', '\t']);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (negated, rest) = match line.strip_prefix('!') {
            Some(r) => (true, r),
            None => (false, line),
        };
        // `\!` and `\#` escape a leading character.
        let rest = rest
            .strip_prefix('\\')
            .filter(|r| r.starts_with('!') || r.starts_with('#'))
            .unwrap_or(rest);
        let (dir_only, rest) = match rest.strip_suffix('/') {
            Some(r) => (true, r),
            None => (false, rest),
        };
        let anchored = rest.contains('/');
        let pattern = rest.trim_start_matches('/');
        if pattern.is_empty() {
            continue;
        }
        rules.push(Rule {
            base: base.to_string(),
            pattern: pattern.to_string(),
            negated,
            dir_only,
            anchored,
        });
    }
    rules
}

/// Read `dir/.gitignore` if present; a missing or unreadable file yields no rules.
pub fn load_dir(dir: &Path, base: &str) -> Vec<Rule> {
    match std::fs::read_to_string(dir.join(".gitignore")) {
        Ok(text) => parse(&text, base),
        Err(_) => Vec::new(),
    }
}

/// Whether `rel` (a `/`-separated path relative to the scan root) is ignored by `rules`.
pub fn is_ignored(rules: &[Rule], rel: &str, is_dir: bool) -> bool {
    let mut ignored = false;
    for rule in rules {
        if rule.matches(rel, is_dir) {
            ignored = !rule.negated;
        }
    }
    ignored
}

impl Rule {
    fn matches(&self, rel: &str, is_dir: bool) -> bool {
        if self.dir_only && !is_dir {
            return false;
        }
        let local = if self.base.is_empty() {
            rel
        } else {
            match rel
                .strip_prefix(self.base.as_str())
                .and_then(|r| r.strip_prefix('/'))
            {
                Some(r) => r,
                None => return false,
            }
        };
        if self.anchored {
            glob_match(self.pattern.as_bytes(), local.as_bytes())
        } else {
            let name = local.rsplit('/').next().unwrap_or(local);
            glob_match(self.pattern.as_bytes(), name.as_bytes())
        }
    }
}

/// Glob matching where `*` and `?` do not cross `/`, and `**` does.
fn glob_match(pat: &[u8], text: &[u8]) -> bool {
    match pat.first() {
        None => text.is_empty(),
        Some(b'*') if pat.get(1) == Some(&b'*') => {
            // `**` matches any run of characters, including `/`. A `/` directly
            // after it may be absorbed so that `a/**/b` also matches `a/b`.
            let mut rest = &pat[2..];
            if rest.first() == Some(&b'/') {
                if glob_match(&rest[1..], text) {
                    return true;
                }
                rest = &rest[1..];
            }
            (0..=text.len()).any(|i| glob_match(rest, &text[i..]))
        }
        Some(b'*') => {
            let rest = &pat[1..];
            let limit = text.iter().position(|&c| c == b'/').unwrap_or(text.len());
            (0..=limit).any(|i| glob_match(rest, &text[i..]))
        }
        Some(b'?') => !text.is_empty() && text[0] != b'/' && glob_match(&pat[1..], &text[1..]),
        Some(b'[') => match (class_end(pat), text.first()) {
            (Some(end), Some(&c)) if c != b'/' => {
                class_contains(&pat[1..end], c) && glob_match(&pat[end + 1..], &text[1..])
            }
            _ => false,
        },
        Some(b'\\') if pat.len() > 1 => {
            !text.is_empty() && text[0] == pat[1] && glob_match(&pat[2..], &text[1..])
        }
        Some(&c) => !text.is_empty() && text[0] == c && glob_match(&pat[1..], &text[1..]),
    }
}

/// Index of the `]` closing the class that starts at `pat[0] == b'['`.
fn class_end(pat: &[u8]) -> Option<usize> {
    let mut i = 1;
    if pat.get(i) == Some(&b'!') || pat.get(i) == Some(&b'^') {
        i += 1;
    }
    if pat.get(i) == Some(&b']') {
        i += 1;
    }
    while i < pat.len() {
        if pat[i] == b']' {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn class_contains(body: &[u8], c: u8) -> bool {
    let (negate, body) = match body.first() {
        Some(b'!') | Some(b'^') => (true, &body[1..]),
        _ => (false, body),
    };
    let mut found = false;
    let mut i = 0;
    while i < body.len() {
        if i + 2 < body.len() && body[i + 1] == b'-' {
            if body[i] <= c && c <= body[i + 2] {
                found = true;
            }
            i += 3;
        } else {
            if body[i] == c {
                found = true;
            }
            i += 1;
        }
    }
    found != negate
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ignored(gitignore: &str, rel: &str, is_dir: bool) -> bool {
        is_ignored(&parse(gitignore, ""), rel, is_dir)
    }

    #[test]
    fn plain_names_match_at_any_depth() {
        assert!(ignored("Scratch.lean", "A/Scratch.lean", false));
        assert!(ignored("gen", "A/gen", true));
        assert!(!ignored("Scratch.lean", "A/Keep.lean", false));
    }

    #[test]
    fn directory_only_patterns_skip_files() {
        assert!(ignored("gen/", "gen", true));
        assert!(!ignored("gen/", "gen", false));
    }

    #[test]
    fn leading_or_inner_slash_anchors_the_pattern() {
        assert!(ignored("/Top.lean", "Top.lean", false));
        assert!(!ignored("/Top.lean", "Sub/Top.lean", false));
        assert!(ignored("Sub/*.lean", "Sub/X.lean", false));
        assert!(!ignored("Sub/*.lean", "Sub/Deep/X.lean", false));
    }

    #[test]
    fn double_star_crosses_directories() {
        assert!(ignored("**/Gen/*.lean", "A/B/Gen/X.lean", false));
        assert!(ignored("a/**/b.lean", "a/b.lean", false));
        assert!(ignored("a/**/b.lean", "a/x/y/b.lean", false));
        assert!(ignored("tmp/**", "tmp/deep/file.lean", false));
    }

    #[test]
    fn negation_reincludes_the_last_match_wins() {
        let rules = "*.lean\n!Keep.lean\n";
        assert!(ignored(rules, "Drop.lean", false));
        assert!(!ignored(rules, "Keep.lean", false));
    }

    #[test]
    fn comments_blank_lines_and_escapes() {
        assert!(!ignored("# Scratch.lean\n\n", "Scratch.lean", false));
        assert!(ignored("\\#Hash.lean", "#Hash.lean", false));
        assert!(ignored("\\!Bang.lean", "!Bang.lean", false));
    }

    #[test]
    fn character_classes() {
        assert!(ignored("Part[0-9].lean", "Part3.lean", false));
        assert!(!ignored("Part[0-9].lean", "PartX.lean", false));
        assert!(ignored("[!A]*.lean", "bee.lean", false));
        assert!(!ignored("[!A]*.lean", "Apple.lean", false));
    }

    #[test]
    fn nested_gitignore_applies_only_below_its_directory() {
        let rules = parse("*.lean", "Sub");
        assert!(is_ignored(&rules, "Sub/X.lean", false));
        assert!(!is_ignored(&rules, "X.lean", false));
        assert!(!is_ignored(&rules, "Other/X.lean", false));
    }
}
