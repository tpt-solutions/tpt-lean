// SPDX-License-Identifier: MIT OR Apache-2.0
//! Search over a prebuilt, compact, read-only index supplied by the caller as JSON bytes.
//!
//! Index format (version 1):
//! `{"version":1,"entries":[{"name":"Nat.add_comm","kind":"theorem","file":"Mathlib/x.lean","line":12,"docstring":"..."}]}`
//! `docstring` is optional. The index is an acceleration artifact only; Lean remains the
//! source of truth for semantics.
//!
//! Ranking is deterministic: score descending, then name, file and line ascending.

use serde::{Deserialize, Serialize};

/// Index format version accepted by [`Index::from_json`].
pub const INDEX_VERSION: u32 = 1;
/// Default number of results returned.
pub const DEFAULT_LIMIT: usize = 20;
/// Hard cap on the number of results returned.
pub const MAX_LIMIT: usize = 200;
/// Maximum docstring characters echoed back in results.
const SNIPPET_CHARS: usize = 240;

#[derive(Debug, Clone, Deserialize)]
pub struct IndexEntry {
    pub name: String,
    pub kind: String,
    pub file: String,
    pub line: u32,
    #[serde(default)]
    pub docstring: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawIndex {
    version: u32,
    entries: Vec<IndexEntry>,
}

/// An entry plus its lowercased fields, prepared once at load time.
struct Prepared {
    entry: IndexEntry,
    lname: String,
    lshort: String,
    ldoc: Option<String>,
}

/// Parsed, read-only search index.
pub struct Index {
    items: Vec<Prepared>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub name: String,
    pub kind: String,
    pub file: String,
    pub line: u32,
    pub score: u32,
    pub docstring: Option<String>,
}

/// Last dot-separated component of a declaration name (`Nat.add_comm` -> `add_comm`).
fn short_name(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

fn snippet(doc: &str) -> String {
    let trimmed = doc.trim();
    if trimmed.chars().count() <= SNIPPET_CHARS {
        return trimmed.to_string();
    }
    let mut out: String = trimmed.chars().take(SNIPPET_CHARS).collect();
    out.push('\u{2026}');
    out
}

fn clamp_limit(limit: Option<usize>) -> usize {
    limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

impl Index {
    /// Parse an index from JSON bytes. Rejects unknown versions and malformed input.
    pub fn from_json(bytes: &[u8]) -> Result<Self, String> {
        let raw: RawIndex = serde_json::from_slice(bytes).map_err(|e| format!("invalid index JSON: {e}"))?;
        if raw.version != INDEX_VERSION {
            return Err(format!(
                "unsupported index version {}; expected {INDEX_VERSION}",
                raw.version
            ));
        }
        let items = raw
            .entries
            .into_iter()
            .map(|entry| {
                let lname = entry.name.to_lowercase();
                let lshort = short_name(&lname).to_string();
                let ldoc = entry.docstring.as_ref().map(|d| d.to_lowercase());
                Prepared {
                    entry,
                    lname,
                    lshort,
                    ldoc,
                }
            })
            .collect();
        Ok(Self { items })
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Rank by declaration name: exact > short-name exact > prefix > short-name prefix > substring.
    pub fn search_symbols(&self, query: &str, limit: Option<usize>) -> Vec<SearchHit> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let scored = self.items.iter().filter_map(|p| {
            let score = if p.lname == q {
                100
            } else if p.lshort == q {
                90
            } else if p.lname.starts_with(&q) {
                70
            } else if p.lshort.starts_with(&q) {
                60
            } else if p.lname.contains(&q) {
                40
            } else {
                return None;
            };
            Some((score, p))
        });
        finish(scored, clamp_limit(limit))
    }

    /// Rank by docstring text. A full-phrase match scores 50, all whitespace-separated terms
    /// present scores 30; a name match adds 20.
    pub fn search_docs(&self, query: &str, limit: Option<usize>) -> Vec<SearchHit> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let terms: Vec<&str> = q.split_whitespace().collect();
        let scored = self.items.iter().filter_map(|p| {
            let doc = p.ldoc.as_deref()?;
            let doc_score = if doc.contains(&q) {
                50
            } else if terms.iter().all(|t| doc.contains(t)) {
                30
            } else {
                return None;
            };
            let name_bonus = if p.lname.contains(&q) { 20 } else { 0 };
            Some((doc_score + name_bonus, p))
        });
        finish(scored, clamp_limit(limit))
    }
}

fn finish<'a>(scored: impl Iterator<Item = (u32, &'a Prepared)>, limit: usize) -> Vec<SearchHit> {
    let mut hits: Vec<(u32, &Prepared)> = scored.collect();
    hits.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.entry.name.cmp(&b.1.entry.name))
            .then_with(|| a.1.entry.file.cmp(&b.1.entry.file))
            .then_with(|| a.1.entry.line.cmp(&b.1.entry.line))
    });
    hits.into_iter()
        .take(limit)
        .map(|(score, p)| SearchHit {
            name: p.entry.name.clone(),
            kind: p.entry.kind.clone(),
            file: p.entry.file.clone(),
            line: p.entry.line,
            score,
            docstring: p.entry.docstring.as_deref().map(snippet),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const INDEX: &str = r#"{"version":1,"entries":[
        {"name":"Nat.add_comm","kind":"theorem","file":"Init/Nat.lean","line":10,"docstring":"Addition on naturals is commutative."},
        {"name":"add_comm","kind":"theorem","file":"Mathlib/Algebra.lean","line":5},
        {"name":"Nat.addLeft","kind":"def","file":"Init/Nat.lean","line":20,"docstring":"Left addition helper."},
        {"name":"List.append","kind":"def","file":"Init/List.lean","line":3,"docstring":"Concatenate two lists."},
        {"name":"Foo.bar_add_baz","kind":"lemma","file":"A.lean","line":1}
    ]}"#;

    fn index() -> Index {
        Index::from_json(INDEX.as_bytes()).unwrap()
    }

    #[test]
    fn exact_beats_prefix_beats_substring() {
        let hits = index().search_symbols("add_comm", None);
        assert_eq!(hits[0].name, "add_comm");
        assert_eq!(hits[0].score, 100);
        assert_eq!(hits[1].name, "Nat.add_comm");
        assert_eq!(hits[1].score, 90);
    }

    #[test]
    fn prefix_and_substring_scores() {
        let hits = index().search_symbols("nat.add", None);
        let names: Vec<(&str, u32)> = hits.iter().map(|h| (h.name.as_str(), h.score)).collect();
        assert_eq!(names, vec![("Nat.addLeft", 70), ("Nat.add_comm", 70)]);
        let sub = index().search_symbols("add_baz", None);
        assert_eq!(sub[0].name, "Foo.bar_add_baz");
        assert_eq!(sub[0].score, 40);
    }

    #[test]
    fn case_insensitive_and_empty_query() {
        assert_eq!(index().search_symbols("LIST.APPEND", None)[0].name, "List.append");
        assert!(index().search_symbols("   ", None).is_empty());
    }

    #[test]
    fn limit_is_applied_and_capped() {
        assert_eq!(index().search_symbols("a", Some(1)).len(), 1);
        assert_eq!(index().search_symbols("a", Some(0)).len(), 1);
        assert!(index().search_symbols("a", Some(10_000)).len() <= MAX_LIMIT);
    }

    #[test]
    fn doc_search_matches_docstrings_only() {
        let hits = index().search_docs("commutative", None);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "Nat.add_comm");
        assert_eq!(hits[0].score, 50);
        // All terms present, not as a phrase.
        let terms = index().search_docs("lists concatenate", None);
        assert_eq!(terms[0].name, "List.append");
        assert_eq!(terms[0].score, 30);
        // Name-only match is not a doc match.
        assert!(index().search_docs("bar_add", None).is_empty());
    }

    #[test]
    fn doc_search_name_bonus_orders_results() {
        let idx = Index::from_json(
            br#"{"version":1,"entries":[
                {"name":"Other.thing","kind":"def","file":"a","line":1,"docstring":"adds numbers"},
                {"name":"Nat.add","kind":"def","file":"b","line":2,"docstring":"adds numbers"}
            ]}"#,
        )
        .unwrap();
        let hits = idx.search_docs("add", None);
        assert_eq!(hits[0].name, "Nat.add");
        assert_eq!(hits[0].score, 70);
        assert_eq!(hits[1].score, 50);
    }

    #[test]
    fn rejects_bad_version_and_bad_json() {
        assert!(Index::from_json(br#"{"version":2,"entries":[]}"#)
            .err()
            .unwrap()
            .contains("unsupported"));
        assert!(Index::from_json(b"not json").is_err());
        assert!(Index::from_json(br#"{"version":1}"#).is_err());
    }

    #[test]
    fn long_docstrings_are_truncated() {
        let long = "x".repeat(1000);
        let json = format!(
            r#"{{"version":1,"entries":[{{"name":"n","kind":"def","file":"f","line":1,"docstring":"{long}"}}]}}"#
        );
        let idx = Index::from_json(json.as_bytes()).unwrap();
        let hits = idx.search_docs("xxx", None);
        let snip = hits[0].docstring.as_ref().unwrap();
        assert_eq!(snip.chars().count(), SNIPPET_CHARS + 1);
    }
}
