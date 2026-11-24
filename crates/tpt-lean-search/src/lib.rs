// SPDX-License-Identifier: MIT OR Apache-2.0
//! Search over the local index: name search (exact, prefix, namespace-aware), docstring
//! full-text search, namespace filtering, `--near` co-location filtering, and ranking.
//!
//! Scores are deterministic and in `0.0..=1.0`. Ranking factors (specification 7.5):
//! exact name match, namespace proximity, docstring relevance, imported modules first,
//! theorem/definition kind, and package (project symbols before dependencies).

use std::collections::{HashMap, HashSet};
use tpt_lean_core::Result;
use tpt_lean_index::{fts_query, Index, SymbolRow};

#[derive(Debug, Clone, Default)]
pub struct SearchOptions {
    pub limit: usize,
    /// Keep only symbols whose namespace equals or lies under this prefix.
    pub namespace: Option<String>,
    /// Keep only symbols whose file also declares a symbol matching each of these terms.
    pub near: Vec<String>,
    /// Modules imported by the current file; symbols from these packages rank higher.
    pub imported_modules: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub name: String,
    pub kind: String,
    pub package: String,
    pub file: String,
    pub line: i64,
    pub docstring: Option<String>,
    pub score: f64,
}

/// Maximum raw score, used to normalise scores into `0..=1`.
const MAX_RAW: f64 = 200.0;

pub fn search(index: &Index, query: &str, opts: &SearchOptions) -> Result<Vec<Hit>> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let limit = if opts.limit == 0 { 10 } else { opts.limit };
    let pool = (limit * 20).max(200);

    // Candidates from name search and from docstring/FTS search.
    let mut candidates: HashMap<i64, (SymbolRow, f64)> = HashMap::new();
    for row in index.symbols_named(query, pool)? {
        let raw = name_score(query, &row);
        candidates.entry(row.id).or_insert((row, 0.0)).1 += raw;
    }
    if let Some(fts) = fts_query(query) {
        for row in index.symbols_fts(&fts, pool)? {
            let raw = if query.contains(char::is_whitespace) {
                30.0
            } else {
                15.0
            };
            candidates.entry(row.id).or_insert((row, 0.0)).1 += raw;
        }
    }

    let near_files = near_filter(index, &opts.near)?;
    let mut hits: Vec<Hit> = candidates
        .into_values()
        .filter(|(row, _)| opts.namespace.as_ref().is_none_or(|ns| in_namespace(row, ns)))
        .filter(|(row, _)| {
            near_files
                .as_ref()
                .is_none_or(|files| files.contains(&(row.package.clone(), row.file.clone())))
        })
        .map(|(row, mut raw)| {
            raw += kind_bonus(&row.kind);
            if row.package == "project" {
                raw += 10.0;
            }
            if opts.imported_modules.iter().any(|m| {
                row.file
                    .replace('/', ".")
                    .trim_end_matches(".lean")
                    .starts_with(m.as_str())
            }) {
                raw += 15.0;
            }
            if let Some(ns) = opts.namespace.as_ref() {
                if row.namespace == *ns {
                    raw += 20.0;
                }
            }
            Hit {
                name: row.name.clone(),
                kind: row.kind.clone(),
                package: row.package.clone(),
                file: row.file.clone(),
                line: row.line,
                docstring: row.docstring.clone(),
                score: (raw / MAX_RAW).min(1.0),
            }
        })
        .collect();

    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.name.len().cmp(&b.name.len()))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.file.cmp(&b.file))
    });
    hits.truncate(limit);
    Ok(hits)
}

/// Name relevance: exact full name > exact short name > prefix of full name > prefix of short name.
fn name_score(query: &str, row: &SymbolRow) -> f64 {
    let short = row.short_name.as_deref().unwrap_or("");
    if row.name == query {
        100.0
    } else if short == query {
        80.0
    } else if row.name.starts_with(query) {
        50.0
    } else if short.starts_with(query) {
        40.0
    } else {
        10.0
    }
}

fn kind_bonus(kind: &str) -> f64 {
    match kind {
        "theorem" | "lemma" => 5.0,
        "def" | "abbrev" => 3.0,
        _ => 0.0,
    }
}

fn in_namespace(row: &SymbolRow, ns: &str) -> bool {
    row.namespace == ns
        || row.namespace.starts_with(&format!("{ns}."))
        || row.name.starts_with(&format!("{ns}."))
}

/// For `--near a b`: the set of (package, file) pairs that declare a symbol matching every term.
fn near_filter(index: &Index, terms: &[String]) -> Result<Option<HashSet<(String, String)>>> {
    if terms.is_empty() {
        return Ok(None);
    }
    let mut acc: Option<HashSet<(String, String)>> = None;
    for term in terms {
        let files: HashSet<(String, String)> = index
            .symbols_named(term, 1000)?
            .into_iter()
            .map(|r| (r.package, r.file))
            .collect();
        acc = Some(match acc {
            None => files,
            Some(prev) => prev.intersection(&files).cloned().collect(),
        });
    }
    Ok(acc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tpt_lean_index::ScanRoot;

    fn fixture() -> Index {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir: PathBuf = std::env::temp_dir().join(format!("tpt-search-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("Nat.lean"),
            "namespace Nat\n/-- Addition is commutative. -/\ntheorem add_comm (n m : Nat) : n + m = m + n := sorry\ntheorem add_assoc : True := trivial\nend Nat\n",
        )
        .unwrap();
        std::fs::write(dir.join("Topo.lean"), "/-- A compact set has a maximum for continuous functions. -/\ntheorem compact_max : True := trivial\n").unwrap();
        let mut idx = Index::open_in_memory().unwrap();
        idx.refresh(&[ScanRoot {
            package: "project".into(),
            path: dir.clone(),
        }])
        .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        idx
    }

    #[test]
    fn exact_name_ranks_first() {
        let idx = fixture();
        let hits = search(
            &idx,
            "Nat.add_comm",
            &SearchOptions {
                limit: 5,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(hits[0].name, "Nat.add_comm");
        assert!(hits[0].score > hits.last().unwrap().score || hits.len() == 1);
    }

    #[test]
    fn short_name_prefix_matches() {
        let idx = fixture();
        let hits = search(
            &idx,
            "add_",
            &SearchOptions {
                limit: 5,
                ..Default::default()
            },
        )
        .unwrap();
        let names: Vec<&str> = hits.iter().map(|h| h.name.as_str()).collect();
        assert!(names.contains(&"Nat.add_comm"));
        assert!(names.contains(&"Nat.add_assoc"));
    }

    #[test]
    fn docstring_search_finds_symbol() {
        let idx = fixture();
        let hits = search(
            &idx,
            "compact maximum continuous",
            &SearchOptions {
                limit: 5,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(hits.iter().any(|h| h.name == "compact_max"));
    }

    #[test]
    fn namespace_filter_restricts_results() {
        let idx = fixture();
        let opts = SearchOptions {
            limit: 10,
            namespace: Some("Nat".into()),
            ..Default::default()
        };
        let hits = search(&idx, "add", &opts).unwrap();
        assert!(!hits.is_empty());
        assert!(hits.iter().all(|h| h.name.starts_with("Nat.")));
    }

    #[test]
    fn scores_are_normalised() {
        let idx = fixture();
        let hits = search(&idx, "add_comm", &SearchOptions::default()).unwrap();
        assert!(hits.iter().all(|h| (0.0..=1.0).contains(&h.score)));
    }

    #[test]
    fn empty_query_returns_nothing() {
        let idx = fixture();
        assert!(search(&idx, "   ", &SearchOptions::default()).unwrap().is_empty());
    }
}
