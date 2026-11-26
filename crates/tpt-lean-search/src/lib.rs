// SPDX-License-Identifier: MIT OR Apache-2.0
//! Search over the local index: name search (exact, prefix, namespace-aware), docstring
//! full-text search, namespace filtering, `--near` co-location filtering, and ranking.
//!
//! Scores are deterministic and in `0.0..=1.0`. Ranking factors (specification 7.5):
//! exact name match, namespace proximity, docstring relevance, imported modules first,
//! theorem/definition kind, and package (project symbols before dependencies).
//!
//! Two more factors are off unless their weight is set ([`RankWeights`], from the `[search]`
//! config section): recency (files whose contents changed more recently in the index rank higher)
//! and dependency distance (modules closer to the origin module, or to the project's root modules,
//! along import edges rank higher). With both weights at zero the ordering is the one without them.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};
use tpt_lean_core::Result;
use tpt_lean_index::{fts_query, module_name_of, Index, SymbolRow};

pub mod types;
pub use types::{search_by_type, type_terms, TypeHit};

/// Weights for the optional ranking factors, in raw score points (the same scale as the name and
/// kind bonuses). A factor in `0..=1` is multiplied by its weight. Zero disables the factor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RankWeights {
    /// Weight of `0.5^(age / half_life)`, where age is the time since the file's contents last
    /// changed in the index.
    pub recency: f64,
    pub recency_half_life_days: f64,
    /// Weight of `1 / (1 + hops)` along import edges. Unreachable modules get no bonus.
    pub distance: f64,
}

impl Default for RankWeights {
    fn default() -> Self {
        Self {
            recency: 0.0,
            recency_half_life_days: 30.0,
            distance: 0.0,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SearchOptions {
    pub limit: usize,
    /// Keep only symbols whose namespace equals or lies under this prefix.
    pub namespace: Option<String>,
    /// Keep only symbols whose file also declares a symbol matching each of these terms.
    pub near: Vec<String>,
    /// Modules imported by the current file; symbols from these packages rank higher.
    pub imported_modules: Vec<String>,
    /// Ranking weights for recency and dependency distance. Default: both off.
    pub weights: RankWeights,
    /// Module the query is run from, for the distance factor. When None, distance is measured
    /// from the project's root modules (project modules that no project module imports).
    pub origin_module: Option<String>,
    /// Clock for recency, in milliseconds since the Unix epoch. None uses the system clock.
    pub now_ms: Option<i64>,
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
    let extra = ExtraFactors::load(index, opts)?;
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
            if let Some(extra) = extra.as_ref() {
                raw += extra.bonus(&row, &opts.weights);
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

/// Data for the optional factors. Loaded only when a weight is non-zero, so the default search
/// makes no extra queries and keeps its ordering.
struct ExtraFactors {
    /// When each `(package, rel_path)` last changed in the index, in epoch milliseconds.
    indexed_at: HashMap<(String, String), i64>,
    now_ms: i64,
    /// Import hops from the origin (or from the root modules) to each project module.
    hops: HashMap<String, usize>,
}

impl ExtraFactors {
    fn load(index: &Index, opts: &SearchOptions) -> Result<Option<Self>> {
        let w = &opts.weights;
        let want_recency = w.recency != 0.0;
        let want_distance = w.distance != 0.0;
        if !want_recency && !want_distance {
            return Ok(None);
        }
        let indexed_at = if want_recency {
            index.file_indexed_at()?
        } else {
            HashMap::new()
        };
        let hops = if want_distance {
            hop_distances(&index.project_import_graph()?, opts.origin_module.as_deref())
        } else {
            HashMap::new()
        };
        Ok(Some(Self {
            indexed_at,
            now_ms: opts.now_ms.unwrap_or_else(now_millis),
            hops,
        }))
    }

    /// Raw points added by the optional factors for one symbol.
    fn bonus(&self, row: &SymbolRow, w: &RankWeights) -> f64 {
        let mut bonus = 0.0;
        if w.recency != 0.0 {
            if let Some(&changed) = self.indexed_at.get(&(row.package.clone(), row.file.clone())) {
                bonus += w.recency * recency_factor(self.now_ms, changed, w.recency_half_life_days);
            }
        }
        if w.distance != 0.0 && row.package == "project" {
            if let Some(&hops) = module_name_of(&row.file).and_then(|m| self.hops.get(&m)) {
                bonus += w.distance / (1.0 + hops as f64);
            }
        }
        bonus
    }
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `0.5^(age / half_life)` with age in days: 1 when changed now, halving every half life.
/// Monotone non-increasing in age. Returns 0 for a half life that is not positive.
fn recency_factor(now_ms: i64, changed_ms: i64, half_life_days: f64) -> f64 {
    if !(half_life_days > 0.0) {
        return 0.0;
    }
    let age_days = (now_ms - changed_ms).max(0) as f64 / 86_400_000.0;
    0.5f64.powf(age_days / half_life_days)
}

/// Breadth-first import hops over project modules. Starts at `origin` when it is a project module;
/// otherwise at the root modules (project modules that no project module imports). Edges follow
/// imports forward and only reach modules in the graph. Modules that are not reached are absent.
fn hop_distances(graph: &HashMap<String, Vec<String>>, origin: Option<&str>) -> HashMap<String, usize> {
    let starts: Vec<&str> = match origin {
        Some(m) if graph.contains_key(m) => vec![m],
        Some(_) => Vec::new(),
        None => {
            let imported: HashSet<&str> = graph
                .values()
                .flat_map(|imports| imports.iter().map(|s| s.as_str()))
                .collect();
            let mut roots: Vec<&str> = graph
                .keys()
                .map(|m| m.as_str())
                .filter(|m| !imported.contains(m))
                .collect();
            roots.sort_unstable();
            roots
        }
    };
    let mut hops: HashMap<String, usize> = HashMap::new();
    let mut queue: VecDeque<&str> = VecDeque::new();
    for start in starts {
        if !hops.contains_key(start) {
            hops.insert(start.to_string(), 0);
            queue.push_back(start);
        }
    }
    while let Some(module) = queue.pop_front() {
        let next_hops = hops[module] + 1;
        for next in graph.get(module).into_iter().flatten() {
            if graph.contains_key(next) && !hops.contains_key(next) {
                hops.insert(next.clone(), next_hops);
                queue.push_back(next.as_str());
            }
        }
    }
    hops
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

    /// A fresh project directory with the given files (relative path, contents), indexed.
    fn project_index(tag: &str, files: &[(&str, &str)]) -> (Index, PathBuf) {
        let dir: PathBuf = std::env::temp_dir().join(format!("tpt-search-rank-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (rel, text) in files {
            std::fs::write(dir.join(rel), text).unwrap();
        }
        let mut idx = Index::open_in_memory().unwrap();
        idx.refresh(&[ScanRoot {
            package: "project".into(),
            path: dir.clone(),
        }])
        .unwrap();
        (idx, dir)
    }

    fn names_of(hits: &[Hit]) -> Vec<&str> {
        hits.iter().map(|h| h.name.as_str()).collect()
    }

    /// `Origin` imports `Near`. `Far` is not connected to either. Both symbols match the prefix,
    /// and by name `shared_far` sorts first.
    fn import_fixture(tag: &str) -> (Index, PathBuf) {
        project_index(
            tag,
            &[
                ("Far.lean", "theorem shared_far : True := trivial\n"),
                ("Near.lean", "theorem shared_near : True := trivial\n"),
                ("Origin.lean", "import Near\ntheorem origin_only : True := trivial\n"),
            ],
        )
    }

    #[test]
    fn zero_weights_keep_the_default_ordering() {
        let (idx, dir) = import_fixture("zero");
        let plain = search(
            &idx,
            "shared_",
            &SearchOptions {
                limit: 10,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(names_of(&plain), ["shared_far", "shared_near"]);
        // Origin and clock are given, but both weights are zero: nothing changes.
        let with_origin = search(
            &idx,
            "shared_",
            &SearchOptions {
                limit: 10,
                origin_module: Some("Origin".into()),
                now_ms: Some(now_millis()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(with_origin, plain);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn distance_weight_moves_the_module_nearer_to_origin_first() {
        let (idx, dir) = import_fixture("distance");
        let weights = RankWeights {
            distance: 10.0,
            ..RankWeights::default()
        };
        let from_origin = search(
            &idx,
            "shared_",
            &SearchOptions {
                limit: 10,
                weights,
                origin_module: Some("Origin".into()),
                ..Default::default()
            },
        )
        .unwrap();
        // Near is one hop from Origin; Far is unreachable and gets no bonus.
        assert_eq!(names_of(&from_origin), ["shared_near", "shared_far"]);

        // Without an origin, distance is measured from the roots: Origin and Far are roots, Near
        // is one hop away. Far keeps the first place.
        let from_roots = search(
            &idx,
            "shared_",
            &SearchOptions {
                limit: 10,
                weights,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(names_of(&from_roots), ["shared_far", "shared_near"]);
        assert!(from_roots[0].score > from_roots[1].score);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recency_weight_moves_recently_changed_files_first() {
        let dir: PathBuf = std::env::temp_dir().join(format!("tpt-search-recency-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Old.lean"), "theorem recency_a_old : True := trivial\n").unwrap();
        let mut idx = Index::open_in_memory().unwrap();
        let roots = [ScanRoot {
            package: "project".into(),
            path: dir.clone(),
        }];
        idx.refresh(&roots).unwrap();
        // Index times are in milliseconds; wait so the second file is clearly newer.
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::fs::write(dir.join("New.lean"), "theorem recency_b_new : True := trivial\n").unwrap();
        idx.refresh(&roots).unwrap();

        let plain = search(
            &idx,
            "recency_",
            &SearchOptions {
                limit: 10,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(names_of(&plain), ["recency_a_old", "recency_b_new"]);

        // A one-second half life makes the 30 ms difference count.
        let weighted = search(
            &idx,
            "recency_",
            &SearchOptions {
                limit: 10,
                weights: RankWeights {
                    recency: 10.0,
                    recency_half_life_days: 1.0 / 86_400.0,
                    distance: 0.0,
                },
                now_ms: Some(now_millis()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(names_of(&weighted), ["recency_b_new", "recency_a_old"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recency_factor_halves_per_half_life_and_is_monotone() {
        let day = 86_400_000i64;
        assert_eq!(recency_factor(1_000, 1_000, 30.0), 1.0);
        assert!((recency_factor(day * 30, 0, 30.0) - 0.5).abs() < 1e-12);
        assert!((recency_factor(day * 60, 0, 30.0) - 0.25).abs() < 1e-12);
        let mut last = 1.0;
        for age_days in 1..100 {
            let f = recency_factor(day * age_days, 0, 30.0);
            assert!(f < last && f > 0.0);
            last = f;
        }
        // A future timestamp counts as age zero.
        assert_eq!(recency_factor(0, 1_000, 30.0), 1.0);
        assert_eq!(recency_factor(day, 0, 0.0), 0.0);
    }

    #[test]
    fn hop_distances_from_origin_and_from_roots() {
        let mut graph: HashMap<String, Vec<String>> = HashMap::new();
        graph.insert("A".into(), vec!["B".into(), "External.Pkg".into()]);
        graph.insert("B".into(), vec!["C".into()]);
        graph.insert("C".into(), vec![]);
        graph.insert("D".into(), vec!["C".into()]);

        let from_a = hop_distances(&graph, Some("A"));
        assert_eq!(from_a.get("A"), Some(&0));
        assert_eq!(from_a.get("B"), Some(&1));
        assert_eq!(from_a.get("C"), Some(&2));
        assert_eq!(from_a.get("D"), None);
        assert_eq!(from_a.get("External.Pkg"), None);

        // Roots are A and D (nothing imports them). C is two hops from A, one from B.
        let from_roots = hop_distances(&graph, None);
        assert_eq!(from_roots.get("A"), Some(&0));
        assert_eq!(from_roots.get("D"), Some(&0));
        assert_eq!(from_roots.get("B"), Some(&1));
        assert_eq!(from_roots.get("C"), Some(&1));

        assert!(hop_distances(&graph, Some("Missing")).is_empty());
    }
}
