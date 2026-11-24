// SPDX-License-Identifier: MIT OR Apache-2.0
//! Report types, JSON (de)serialization, ranking, text rendering, comparison and suggestions.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use tpt_lean_core::{Error, ErrorCode, Result};

/// Version of the JSON report layout. Bump when fields change meaning.
pub const FORMAT_VERSION: u32 = 1;

/// Profile of one Lean file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileReport {
    pub format_version: u32,
    /// The profiled file, as the caller gave it.
    pub file: String,
    /// Wall-clock time of the `lean` process, in milliseconds.
    pub wall_ms: f64,
    pub lean_exit_code: Option<i32>,
    /// Number of error diagnostics Lean printed. Timings are incomplete when this is non-zero.
    pub lean_errors: usize,
    /// Sum of declaration totals. Excludes imports and startup, which are not attributed.
    pub declared_ms: f64,
    pub declarations: Vec<DeclTiming>,
    /// Totals from the `cumulative profiling times` block, keyed by Lean's label.
    pub categories: BTreeMap<String, f64>,
    pub tactics: Vec<TacticTiming>,
    /// Peak resident memory of the Lean process tree during the run, in bytes. Sampled, so a
    /// short spike can be missed. `None` when it was not measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_memory_bytes: Option<u64>,
    /// Per-import load costs. Empty unless the import timing was requested.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub import_timings: Vec<ImportTiming>,
    pub suggestions: Vec<String>,
    /// Things this report does not measure, so readers do not mistake absence for zero.
    pub notes: Vec<String>,
}

/// Timing of one top-level declaration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeclTiming {
    /// `theorem`, `def`, `instance`, `example`, ... or `decl` when only the proof was seen.
    pub kind: String,
    pub name: Option<String>,
    /// 1-based line in the original file, or 0 when it could not be located.
    pub line: u32,
    pub total_ms: f64,
    /// Elaboration of the declaration header and synchronous parts.
    pub elab_ms: f64,
    /// Asynchronous proof elaboration.
    pub proof_ms: f64,
    /// Kernel type checking.
    pub kernel_ms: f64,
    pub main_cost: Option<String>,
    pub main_cost_ms: f64,
}

/// Cost of loading one direct import, measured by `run::import_timings`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportTiming {
    pub module: String,
    /// 1-based line of the import in the profiled file.
    pub line: u32,
    /// Extra wall time over an empty file, in milliseconds. One run per import, so treat small
    /// differences as noise. Includes the import's transitive closure.
    pub load_ms: f64,
    /// Set when Lean could not load the import; `load_ms` is then zero.
    pub error: Option<String>,
}

/// Graphviz DOT for the direct imports of the profiled file, with each import's measured load
/// cost on its edge. Imports at or above half the largest cost are drawn in red, those at or
/// above a fifth in orange, the rest in grey; imports Lean could not load are dashed. This is the
/// first level only: each cost already includes the import's own closure.
pub fn import_graph_dot(report: &ProfileReport) -> String {
    let max = report
        .import_timings
        .iter()
        .map(|t| t.load_ms)
        .fold(0.0_f64, f64::max);
    let mut out = String::new();
    let _ = writeln!(out, "digraph imports {{");
    let _ = writeln!(out, "  rankdir=LR;");
    let _ = writeln!(out, "  node [shape=box, fontname=\"Helvetica\"];");
    // Label the profiled file by its name; the full path is in the report itself.
    let name = std::path::Path::new(&report.file)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| report.file.clone());
    let file = dot_escape(&name);
    let _ = writeln!(out, "  \"{file}\" [style=bold];");
    for imp in &report.import_timings {
        let module = dot_escape(&imp.module);
        match &imp.error {
            Some(err) => {
                let label = dot_escape(&format!("{}\nnot loaded: {err}", imp.module));
                let _ = writeln!(
                    out,
                    "  \"{module}\" [label=\"{label}\", style=dashed, color=grey];"
                );
                let _ = writeln!(out, "  \"{file}\" -> \"{module}\" [style=dashed, color=grey];");
            }
            None => {
                let colour = if max > 0.0 && imp.load_ms >= max / 2.0 {
                    "red"
                } else if max > 0.0 && imp.load_ms >= max / 5.0 {
                    "orange"
                } else {
                    "grey"
                };
                let cost = format_ms(imp.load_ms);
                let label = dot_escape(&format!("{}\n{cost}", imp.module));
                let _ = writeln!(out, "  \"{module}\" [label=\"{label}\"];");
                let _ = writeln!(
                    out,
                    "  \"{file}\" -> \"{module}\" [label=\"{cost}\", color={colour}, penwidth={}];",
                    if colour == "grey" { "1" } else { "2" }
                );
            }
        }
    }
    let _ = writeln!(out, "}}");
    out
}

/// Escape a string for a double-quoted DOT identifier or label.
fn dot_escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// Aggregated time of one tactic across the file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TacticTiming {
    pub name: String,
    pub total_ms: f64,
    pub count: usize,
}

/// Serialize a report as pretty JSON.
pub fn to_json(report: &ProfileReport) -> Result<String> {
    serde_json::to_string_pretty(report)
        .map_err(|e| Error::new(ErrorCode::Internal, format!("serializing profile report: {e}")))
}

/// Parse a report from JSON produced by [`to_json`].
pub fn from_json(text: &str) -> Result<ProfileReport> {
    let report: ProfileReport =
        serde_json::from_str(text).map_err(|e| Error::invalid(format!("not a profile report: {e}")))?;
    if report.format_version != FORMAT_VERSION {
        return Err(Error::invalid(format!(
            "unsupported profile report format {} (expected {FORMAT_VERSION})",
            report.format_version
        )));
    }
    Ok(report)
}

/// Read a report from a JSON file.
pub fn read_report(path: &std::path::Path) -> Result<ProfileReport> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| Error::new(ErrorCode::Io, format!("reading {}: {e}", path.display())))?;
    from_json(&text)
}

/// The `n` declarations with the largest total time, slowest first.
pub fn slowest(report: &ProfileReport, n: usize) -> Vec<DeclTiming> {
    let mut decls = report.declarations.clone();
    decls.sort_by(|a, b| b.total_ms.total_cmp(&a.total_ms));
    decls.truncate(n);
    decls
}

/// Human-readable duration: seconds with one decimal from one second upwards, else milliseconds.
pub fn format_ms(ms: f64) -> String {
    if ms >= 1000.0 {
        format!("{:.1}s", ms / 1000.0)
    } else {
        format!("{ms:.1}ms")
    }
}

/// Text report: header, the `n` slowest declarations, and suggested actions.
pub fn render_report(report: &ProfileReport, n: usize) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "File: {}", report.file);
    let _ = writeln!(out, "Wall time: {}", format_ms(report.wall_ms));
    let _ = writeln!(
        out,
        "Declared time: {} (imports and startup are not attributed)",
        format_ms(report.declared_ms)
    );
    if report.lean_errors > 0 {
        let _ = writeln!(
            out,
            "Lean reported {} error(s); timings may be incomplete",
            report.lean_errors
        );
    }
    if !report.declarations.is_empty() {
        let _ = writeln!(out, "\nSlowest declarations:");
        for (i, d) in slowest(report, n).iter().enumerate() {
            let _ = writeln!(
                out,
                "{}. {} {}",
                i + 1,
                d.kind,
                d.name.as_deref().unwrap_or("(anonymous)")
            );
            if d.line > 0 {
                let _ = writeln!(out, "   File: {}:{}", report.file, d.line);
            } else {
                let _ = writeln!(out, "   File: {}", report.file);
            }
            let _ = writeln!(out, "   Time: {}", format_ms(d.total_ms));
            if let Some(cost) = &d.main_cost {
                let _ = writeln!(out, "   Main cost: {cost}");
            }
        }
    } else {
        out.push_str("\nNo declarations were reported.\n");
    }
    if !report.tactics.is_empty() {
        let _ = writeln!(out, "\nTactics:");
        for t in report.tactics.iter().take(n) {
            let _ = writeln!(out, "  {} x{}: {}", t.name, t.count, format_ms(t.total_ms));
        }
    }
    if let Some(bytes) = report.peak_memory_bytes {
        let _ = writeln!(
            out,
            "Peak memory (Lean process tree, sampled): {:.1} MiB",
            bytes as f64 / (1024.0 * 1024.0)
        );
    }
    if !report.import_timings.is_empty() {
        let _ = writeln!(out, "\nImports (slowest to load, approximate):");
        let mut sorted: Vec<&ImportTiming> = report.import_timings.iter().collect();
        sorted.sort_by(|a, b| b.load_ms.total_cmp(&a.load_ms));
        for imp in sorted.into_iter().take(n) {
            match &imp.error {
                Some(err) => {
                    let _ = writeln!(out, "  {} (line {}): not loaded: {err}", imp.module, imp.line);
                }
                None => {
                    let _ = writeln!(
                        out,
                        "  {} (line {}): {}",
                        imp.module,
                        imp.line,
                        format_ms(imp.load_ms)
                    );
                }
            }
        }
    }
    if !report.suggestions.is_empty() {
        out.push_str("\nSuggested actions:\n");
        for s in &report.suggestions {
            let _ = writeln!(out, "- {s}");
        }
    }
    if !report.notes.is_empty() {
        out.push_str("\nNot measured:\n");
        for note in &report.notes {
            let _ = writeln!(out, "- {note}");
        }
    }
    out
}

/// Compare two reports. Declarations are matched by kind and name (anonymous ones by kind and
/// line), so reordering does not count as a change.
pub fn compare(a: &ProfileReport, b: &ProfileReport) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Before: {}", a.file);
    let _ = writeln!(out, "After:  {}", b.file);
    let _ = writeln!(out, "\nWall time: {}", delta_line(a.wall_ms, b.wall_ms));
    let _ = writeln!(out, "Declared: {}", delta_line(a.declared_ms, b.declared_ms));

    let keys: BTreeSet<&str> = a
        .categories
        .keys()
        .chain(b.categories.keys())
        .map(String::as_str)
        .collect();
    let mut cats: Vec<(&str, f64, f64)> = keys
        .into_iter()
        .map(|k| {
            (
                k,
                a.categories.get(k).copied().unwrap_or(0.0),
                b.categories.get(k).copied().unwrap_or(0.0),
            )
        })
        .filter(|(_, x, y)| (x - y).abs() > 0.0)
        .collect();
    cats.sort_by(|x, y| (y.2 - y.1).abs().total_cmp(&(x.2 - x.1).abs()));
    if !cats.is_empty() {
        out.push_str("\nCategories (changed):\n");
        for (k, x, y) in cats {
            let _ = writeln!(out, "  {k}: {}", delta_line(x, y));
        }
    }

    let key = |d: &DeclTiming| match &d.name {
        Some(n) => format!("{} {n}", d.kind),
        None => format!("{} (line {})", d.kind, d.line),
    };
    let before: BTreeMap<String, &DeclTiming> = a.declarations.iter().map(|d| (key(d), d)).collect();
    let after: BTreeMap<String, &DeclTiming> = b.declarations.iter().map(|d| (key(d), d)).collect();

    let mut matched: Vec<(String, f64, f64)> = Vec::new();
    let mut only_before = Vec::new();
    let mut only_after = Vec::new();
    for (k, d) in &before {
        match after.get(k) {
            Some(e) => matched.push((k.clone(), d.total_ms, e.total_ms)),
            None => only_before.push(k.clone()),
        }
    }
    for k in after.keys() {
        if !before.contains_key(k) {
            only_after.push(k.clone());
        }
    }
    matched.sort_by(|x, y| (y.2 - y.1).abs().total_cmp(&(x.2 - x.1).abs()));
    if !matched.is_empty() {
        out.push_str("\nDeclarations (matched):\n");
        for (k, x, y) in &matched {
            let _ = writeln!(out, "  {k}: {}", delta_line(*x, *y));
        }
    }
    if !only_before.is_empty() {
        out.push_str("\nOnly in before:\n");
        for k in &only_before {
            let _ = writeln!(out, "  {k}: {}", format_ms(before[k].total_ms));
        }
    }
    if !only_after.is_empty() {
        out.push_str("\nOnly in after:\n");
        for k in &only_after {
            let _ = writeln!(out, "  {k}: {}", format_ms(after[k].total_ms));
        }
    }
    out
}

fn delta_line(before: f64, after: f64) -> String {
    let diff = after - before;
    let pct = if before > 0.0 {
        format!(" ({:+.1}%)", diff / before * 100.0)
    } else {
        String::new()
    };
    format!(
        "{} -> {}: {}{}{}",
        format_ms(before),
        format_ms(after),
        if diff >= 0.0 { "+" } else { "-" },
        format_ms(diff.abs()),
        pct
    )
}

/// Suggested actions derived from the measured data. Only rules that the data supports are used.
pub fn suggest(report: &ProfileReport) -> Vec<String> {
    let mut out = Vec::new();
    for d in slowest(report, 5).iter().filter(|d| d.total_ms >= 500.0) {
        let name = d
            .name
            .clone()
            .unwrap_or_else(|| format!("{} at line {}", d.kind, d.line));
        let advice = match d.main_cost.as_deref() {
            Some("typeclass resolution") => {
                "add explicit instance arguments or local instances, or reduce instance search"
            }
            Some("simp/search automation") => {
                "pass explicit lemma arguments or use `simp only` with a smaller simp set"
            }
            Some("unification (isDefEq)") => {
                "add explicit type annotations or lemma arguments to reduce unification"
            }
            Some("kernel type checking") => {
                "split into smaller lemmas and avoid large `decide` or `rfl` proofs"
            }
            Some("reduction (whnf)") => {
                "avoid reducing large terms by `decide` or `rfl`, or mark definitions `@[irreducible]`"
            }
            _ => "split into smaller lemmas to shorten elaboration",
        };
        out.push(format!("`{name}` takes {}: {advice}", format_ms(d.total_ms)));
    }

    if let Some(top_tactic) = report.tactics.first() {
        if report.declared_ms > 0.0 && top_tactic.total_ms / report.declared_ms >= 0.2 {
            out.push(format!(
                "tactic `{}` accounts for {:.0}% of declared time; narrow its arguments or replace it",
                top_tactic.name,
                top_tactic.total_ms / report.declared_ms * 100.0
            ));
        }
    }

    if report.wall_ms >= 1000.0 && report.declared_ms < report.wall_ms * 0.5 {
        out.push(format!(
            "only {:.0}% of wall time is inside declarations; the rest is import and startup, so reduce imports",
            report.declared_ms / report.wall_ms * 100.0
        ));
    }
    if report.lean_errors > 0 {
        out.push(format!(
            "Lean reported {} error(s); fix them before trusting these timings",
            report.lean_errors
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decl(name: &str, total: f64, cost: Option<&str>) -> DeclTiming {
        DeclTiming {
            kind: "theorem".into(),
            name: Some(name.into()),
            line: 3,
            total_ms: total,
            elab_ms: total,
            proof_ms: 0.0,
            kernel_ms: 0.0,
            main_cost: cost.map(String::from),
            main_cost_ms: total / 2.0,
        }
    }

    fn report(file: &str, wall: f64, decls: Vec<DeclTiming>) -> ProfileReport {
        let declared = decls.iter().map(|d| d.total_ms).sum();
        ProfileReport {
            format_version: FORMAT_VERSION,
            file: file.into(),
            wall_ms: wall,
            lean_exit_code: Some(0),
            lean_errors: 0,
            declared_ms: declared,
            declarations: decls,
            categories: BTreeMap::from([("elaboration".to_string(), 9.5)]),
            tactics: vec![],
            peak_memory_bytes: None,
            import_timings: vec![],
            suggestions: vec![],
            notes: vec![],
        }
    }

    #[test]
    fn import_graph_colours_by_cost() {
        let mut r = report("Main.lean", 100.0, vec![]);
        r.import_timings = vec![
            ImportTiming {
                module: "Big".into(),
                line: 1,
                load_ms: 800.0,
                error: None,
            },
            ImportTiming {
                module: "Mid".into(),
                line: 2,
                load_ms: 300.0,
                error: None,
            },
            ImportTiming {
                module: "Small".into(),
                line: 3,
                load_ms: 10.0,
                error: None,
            },
            ImportTiming {
                module: "Missing".into(),
                line: 4,
                load_ms: 0.0,
                error: Some("unknown module".into()),
            },
        ];
        let dot = import_graph_dot(&r);
        assert!(dot.starts_with("digraph imports {"), "{dot}");
        assert!(
            dot.contains(r#""Main.lean" -> "Big" [label="800.0ms", color=red"#),
            "{dot}"
        );
        assert!(
            dot.contains(r#""Main.lean" -> "Mid" [label="300.0ms", color=orange"#),
            "{dot}"
        );
        assert!(
            dot.contains(r#""Main.lean" -> "Small" [label="10.0ms", color=grey"#),
            "{dot}"
        );
        assert!(dot.contains("style=dashed"), "{dot}");
        assert!(dot.contains("not loaded: unknown module"), "{dot}");
        assert!(dot.trim_end().ends_with('}'), "{dot}");
    }

    #[test]
    fn dot_labels_escape_quotes_and_backslashes() {
        let mut r = report(r#"Main "x".lean"#, 1.0, vec![]);
        r.import_timings = vec![ImportTiming {
            module: r#"A\B"#.into(),
            line: 1,
            load_ms: 5.0,
            error: None,
        }];
        let dot = import_graph_dot(&r);
        assert!(dot.contains(r#""Main \"x\".lean""#), "{dot}");
        assert!(dot.contains(r#""A\\B" [label="A\\B\n5.0ms"]"#), "{dot}");
    }

    #[test]
    fn slowest_orders_and_truncates() {
        let r = report(
            "A.lean",
            100.0,
            vec![decl("a", 1.0, None), decl("b", 7.0, None), decl("c", 3.0, None)],
        );
        let top = slowest(&r, 2);
        assert_eq!(
            top.iter().map(|d| d.name.clone().unwrap()).collect::<Vec<_>>(),
            ["b", "c"]
        );
        assert_eq!(slowest(&r, 10).len(), 3);
    }

    #[test]
    fn json_round_trip_and_version_check() {
        let r = report(
            "A.lean",
            120.0,
            vec![decl("a", 18400.0, Some("simp/search automation"))],
        );
        let text = to_json(&r).unwrap();
        assert_eq!(from_json(&text).unwrap(), r);
        let bad = text.replace("\"format_version\": 1", "\"format_version\": 99");
        assert!(from_json(&bad).is_err());
        assert!(from_json("{}").is_err());
    }

    #[test]
    fn render_mentions_ranking_and_cost() {
        let r = report(
            "src/Topology.lean",
            20000.0,
            vec![decl("big", 18400.0, Some("simp/search automation"))],
        );
        let text = render_report(&r, 5);
        assert!(text.contains("1. theorem big"));
        assert!(text.contains("File: src/Topology.lean:3"));
        assert!(text.contains("Time: 18.4s"));
        assert!(text.contains("Main cost: simp/search automation"));
    }

    #[test]
    fn compare_reports_deltas_and_unmatched() {
        let a = report(
            "before.lean",
            1000.0,
            vec![decl("x", 100.0, None), decl("gone", 5.0, None)],
        );
        let b = report(
            "after.lean",
            500.0,
            vec![decl("x", 50.0, None), decl("new", 7.0, None)],
        );
        let text = compare(&a, &b);
        assert!(
            text.contains("Wall time: 1.0s -> 500.0ms: -500.0ms (-50.0%)"),
            "{text}"
        );
        assert!(
            text.contains("theorem x: 100.0ms -> 50.0ms: -50.0ms (-50.0%)"),
            "{text}"
        );
        assert!(text.contains("Only in before:\n  theorem gone: 5.0ms"), "{text}");
        assert!(text.contains("Only in after:\n  theorem new: 7.0ms"), "{text}");
    }

    #[test]
    fn suggestions_follow_data() {
        let mut r = report(
            "A.lean",
            10000.0,
            vec![decl("slow", 4000.0, Some("typeclass resolution"))],
        );
        r.tactics = vec![TacticTiming {
            name: "simp".into(),
            total_ms: 3000.0,
            count: 4,
        }];
        let s = suggest(&r);
        assert!(
            s.iter()
                .any(|x| x.contains("`slow` takes 4.0s") && x.contains("instance")),
            "{s:?}"
        );
        assert!(
            s.iter().any(|x| x.contains("tactic `simp` accounts for")),
            "{s:?}"
        );
        assert!(s.iter().any(|x| x.contains("only 40% of wall time")), "{s:?}");
    }
}
