// SPDX-License-Identifier: MIT OR Apache-2.0
//! Lemma candidates for a goal, taken from the project index. Read-only: the index is only
//! queried, never refreshed.

use std::collections::HashMap;
use tpt_lean_core::Result;
use tpt_lean_index::Index;
use tpt_lean_search::{search, Hit, SearchOptions};

/// Most identifiers taken from one goal. Each one costs one index query.
const MAX_TERMS: usize = 8;

/// Words that appear in almost every goal and would only add noise to the search.
const STOP_WORDS: &[&str] = &[
    "fun", "forall", "let", "have", "show", "from", "then", "else", "with", "by", "this", "the", "True",
    "False", "Prop", "Type", "Sort", "Nat", "Int", "Bool", "List", "Eq", "HEq", "And", "Or", "Not", "Iff",
    "Exists",
];

/// Candidate lemma names for `goal`, best first, at most `limit` (default 10 when 0).
///
/// Identifiers in the goal are searched one by one and their scores summed, so a symbol that
/// matches several goal terms ranks higher. Theorems and lemmas are boosted over other kinds.
pub fn suggest_lemmas(index: &Index, goal: &str, limit: usize) -> Result<Vec<Hit>> {
    let limit = if limit == 0 { 10 } else { limit };
    let opts = SearchOptions {
        limit: limit * 2,
        ..SearchOptions::default()
    };
    let mut merged: HashMap<(String, String), Hit> = HashMap::new();
    for term in goal_terms(goal) {
        for hit in search(index, &term, &opts)? {
            let key = (hit.name.clone(), hit.file.clone());
            merged
                .entry(key)
                .and_modify(|h| h.score += hit.score)
                .or_insert(hit);
        }
    }
    let mut hits: Vec<Hit> = merged
        .into_values()
        .map(|mut h| {
            if h.kind == "theorem" || h.kind == "lemma" {
                h.score *= 1.5;
            }
            h
        })
        .collect();
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.name.cmp(&b.name))
    });
    hits.truncate(limit);
    Ok(hits)
}

/// Identifier-like words of a goal, in order of appearance, without duplicates or stop words.
pub fn goal_terms(goal: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in goal.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.' || c == '\'')) {
        let word = word.trim_matches('.');
        if word.chars().count() < 2 || !word.chars().any(char::is_alphabetic) {
            continue;
        }
        if STOP_WORDS.contains(&word) || out.iter().any(|w| w == word) {
            continue;
        }
        out.push(word.to_string());
        if out.len() == MAX_TERMS {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terms_skip_noise_and_duplicates() {
        let terms = goal_terms("⊢ a + b = b + a ∧ Nat.add_comm a b = Nat.add_comm a b");
        assert_eq!(terms, vec!["Nat.add_comm".to_string()]);
        let terms = goal_terms("⊢ length (xs ++ ys) = length xs + length ys");
        assert_eq!(terms, vec!["length", "xs", "ys"]);
    }

    #[test]
    fn terms_are_capped() {
        let goal = "alpha beta gamma delta epsilon zeta eta theta iota kappa";
        assert_eq!(goal_terms(goal).len(), MAX_TERMS);
    }

    #[test]
    fn empty_goal_yields_no_candidates() {
        let index = Index::open_in_memory().unwrap();
        assert!(suggest_lemmas(&index, "", 5).unwrap().is_empty());
    }
}

#[cfg(test)]
mod index_tests {
    use super::*;
    use tpt_lean_index::ScanRoot;

    #[test]
    fn ranks_declarations_matching_goal_terms() {
        let dir = std::env::temp_dir().join(format!("tpt-lemmas-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("Arith.lean"),
            "namespace Arith\n\ntheorem add_comm' (a b : Nat) : a + b = b + a := Nat.add_comm a b\n\ndef double (n : Nat) : Nat := n + n\n\nend Arith\n",
        )
        .unwrap();
        let mut index = Index::open_in_memory().unwrap();
        index
            .refresh(&[ScanRoot {
                package: "proj".into(),
                path: dir.clone(),
            }])
            .unwrap();
        let hits = suggest_lemmas(&index, "⊢ add_comm a b", 5).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!hits.is_empty(), "expected candidates");
        assert!(hits[0].name.contains("add_comm"), "{hits:?}");
    }
}
