// SPDX-License-Identifier: MIT OR Apache-2.0
//! Type-shaped search: declarations whose pretty-printed type mentions every identifier in a
//! pattern, such as `Nat -> Nat` or `List ?a -> Nat`.
//!
//! The pattern is split into identifier tokens (letters, digits, `_`, `'`, `.`). Arrows,
//! brackets and other punctuation are ignored, so word order is not required, but it raises the
//! score when the tokens appear in the same order in the type. Only declarations that the sidecar
//! has typed are searchable; run `tpt-lean index sidecar` first.

use crate::SearchOptions;
use std::collections::HashMap;
use tpt_lean_core::Result;
use tpt_lean_index::Index;

/// One declaration whose type matched the pattern.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeHit {
    pub name: String,
    pub kind: String,
    pub package: String,
    pub file: String,
    pub type_text: String,
    pub score: f64,
}

/// Identifier tokens of a type pattern, in order. Repeats are kept: `Nat → Nat` needs two `Nat`
/// tokens in the type.
pub fn type_terms(pattern: &str) -> Vec<String> {
    identifier_tokens(pattern).map(str::to_string).collect()
}

fn identifier_tokens(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '\'' || c == '.'))
        .filter(|t| !t.is_empty())
}

/// Whether every term occurs in `type_text` as a whole token, at least as often as in the pattern.
fn has_all_terms(type_text: &str, terms: &[String]) -> bool {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for tok in identifier_tokens(type_text) {
        *counts.entry(tok).or_insert(0) += 1;
    }
    let mut need: HashMap<&str, usize> = HashMap::new();
    for t in terms {
        *need.entry(t.as_str()).or_insert(0) += 1;
    }
    need.iter()
        .all(|(t, n)| counts.get(t).copied().unwrap_or(0) >= *n)
}

/// Whether `terms` occur in `type_text` as whole tokens, in the given order.
fn ordered(type_text: &str, terms: &[String]) -> bool {
    let mut want = terms.iter();
    let mut next = want.next();
    for tok in identifier_tokens(type_text) {
        if let Some(t) = next {
            if tok == t {
                next = want.next();
            }
        }
    }
    next.is_none()
}

/// Search declaration types. Every identifier in `pattern` must appear as a whole token in the
/// type, as often as it appears in the pattern. `opts.namespace` keeps only names under that namespace; `opts.limit` caps the results.
pub fn search_by_type(index: &Index, pattern: &str, opts: &SearchOptions) -> Result<Vec<TypeHit>> {
    let terms = type_terms(pattern);
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let limit = if opts.limit == 0 { 10 } else { opts.limit };
    let pool = (limit * 20).max(200);
    let rows = index.declarations_with_type_terms(&terms, pool)?;
    let mut hits: Vec<TypeHit> = rows
        .into_iter()
        .filter(|r| has_all_terms(&r.type_text, &terms))
        .filter(|r| match &opts.namespace {
            None => true,
            Some(ns) => r.name == *ns || r.name.starts_with(&format!("{ns}.")),
        })
        .map(|r| {
            let mut score = 0.6;
            if ordered(&r.type_text, &terms) {
                score += 0.2;
            }
            if r.package == "project" {
                score += 0.1;
            }
            if matches!(r.kind.as_str(), "theorem" | "lemma" | "def" | "abbrev") {
                score += 0.1;
            }
            TypeHit {
                name: r.name,
                kind: r.kind,
                package: r.package,
                file: r.file,
                type_text: r.type_text,
                score: f64::min(score, 1.0),
            }
        })
        .collect();
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.name.len().cmp(&b.name.len()))
            .then_with(|| a.name.cmp(&b.name))
    });
    hits.truncate(limit);
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terms_ignore_arrows_and_punctuation() {
        assert_eq!(type_terms("Nat → Nat"), vec!["Nat", "Nat"]);
        assert_eq!(type_terms("List ?a -> Nat"), vec!["List", "a", "Nat"]);
        assert_eq!(type_terms("Nat.add Nat.add"), vec!["Nat.add", "Nat.add"]);
        assert!(type_terms(" -> ").is_empty());
    }

    #[test]
    fn ordered_checks_token_sequence() {
        let terms = type_terms("Nat Bool");
        assert!(ordered("Nat → Bool", &terms));
        assert!(!ordered("Bool → Nat", &terms));
    }

    fn typed_index() -> Index {
        use tpt_lean_index::{DeclRecord, ScanRoot};
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("tpt-types-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("Nat.lean"),
            "theorem add_comm : True := trivial
",
        )
        .unwrap();
        std::fs::write(
            dir.join("Lst.lean"),
            "theorem length_map : True := trivial
",
        )
        .unwrap();
        let mut idx = Index::open_in_memory().unwrap();
        idx.refresh(&[ScanRoot {
            package: "project".into(),
            path: dir.clone(),
        }])
        .unwrap();
        let tc = "leanprover/lean4:v4.36.0";
        let rec = |name: &str, kind: &str, ty: &str| DeclRecord {
            name: name.into(),
            kind: kind.into(),
            type_text: ty.into(),
            docstring: None,
        };
        idx.store_declarations(
            "Nat",
            tc,
            1,
            &[
                rec("Nat.add_comm", "theorem", "∀ (n m : Nat), n + m = m + n"),
                rec("Nat.succ", "def", "Nat → Nat"),
            ],
        )
        .unwrap();
        idx.store_declarations(
            "Lst",
            tc,
            1,
            &[rec("Lst.length_map", "theorem", "List Nat → Nat.count")],
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        idx
    }

    #[test]
    fn finds_declarations_by_type_tokens() {
        let idx = typed_index();
        // Two `Nat` tokens are required, so the single-`Nat` declarations do not match.
        let hits = search_by_type(
            &idx,
            "Nat → Nat",
            &SearchOptions {
                limit: 10,
                ..Default::default()
            },
        )
        .unwrap();
        let names: Vec<&str> = hits.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, vec!["Nat.succ"]);
        let by_count = search_by_type(
            &idx,
            "Nat",
            &SearchOptions {
                limit: 10,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(by_count.len(), 3);
    }

    #[test]
    fn whole_tokens_only_and_order_ranks_higher() {
        let idx = typed_index();
        // `Nat.count` is a different token from `Nat`.
        let hits = search_by_type(
            &idx,
            "Nat.count",
            &SearchOptions {
                limit: 10,
                ..Default::default()
            },
        )
        .unwrap();
        let names: Vec<&str> = hits.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, vec!["Lst.length_map"]);
        let ordered = search_by_type(
            &idx,
            "Nat n",
            &SearchOptions {
                limit: 10,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(ordered.first().map(|h| h.name.as_str()), Some("Nat.add_comm"));
    }

    #[test]
    fn namespace_filter_and_empty_pattern() {
        let idx = typed_index();
        let opts = SearchOptions {
            limit: 10,
            namespace: Some("Lst".into()),
            ..Default::default()
        };
        let hits = search_by_type(&idx, "Nat.count", &opts).unwrap();
        assert_eq!(hits.len(), 1);
        let other = SearchOptions {
            limit: 10,
            namespace: Some("Nat".into()),
            ..Default::default()
        };
        assert!(search_by_type(&idx, "Nat.count", &other).unwrap().is_empty());
        assert!(search_by_type(&idx, "->", &SearchOptions::default())
            .unwrap()
            .is_empty());
    }
}
