// SPDX-License-Identifier: MIT OR Apache-2.0
//! Turns Lean profiler output into declaration timings, cost categories and tactic totals.
//!
//! Attribution rules, derived from real output (see `tests/fixtures`):
//! - `[Elab.command]` roots carry a declaration's header elaboration (`def`, `theorem`, ...).
//! - Proofs of theorems are elaborated asynchronously in `[Elab.async] elaborating proof of NAME`
//!   roots, which are attached to the declaration of the same name.
//! - Kernel checks appear as `[Kernel] typechecking declarations [NAME._proof_1_1, ...]`; the
//!   time is attributed to the declaration whose name is the part before `._proof`.
//! - The main cost of a declaration is the trace tag with the largest self time within its
//!   subtrees, after structural tags are excluded.

use crate::report::{DeclTiming, TacticTiming};
use crate::trace::{parse_decl_header, parse_summary, parse_trace, subtree, Node};
use std::collections::BTreeMap;

/// Everything extracted from one profiled run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Analysis {
    pub declarations: Vec<DeclTiming>,
    pub categories: BTreeMap<String, f64>,
    pub tactics: Vec<TacticTiming>,
}

/// A declaration being assembled from trace roots.
#[derive(Debug, Default)]
struct DeclBuild {
    kind: String,
    name: Option<String>,
    command: Option<usize>,
    proof: Option<usize>,
    kernel: Vec<usize>,
}

const STRUCTURAL_TAGS: &[&str] = &[
    "Elab.command",
    "Elab.async",
    "Elab.definition",
    "Elab.definition.header",
    "Elab.definition.value",
    "Elab.step",
];

const TACTIC_PREFIX: &str = "Lean.Parser.Tactic.";

/// Analyse combined Lean output (stdout followed by stderr) for the instrumented file, with
/// `source` being the original file text used to find declaration lines.
pub fn analyze(output: &str, source: &str) -> Analysis {
    let nodes = parse_trace(output);
    let summary = parse_summary(output);
    let builds = collect_declarations(&nodes);
    let lines: Vec<&str> = source.lines().collect();

    let mut ordinals: BTreeMap<String, usize> = BTreeMap::new();
    let mut declarations = Vec::with_capacity(builds.len());
    for build in &builds {
        let ordinal_key = build.kind.clone();
        let ordinal = if build.name.is_none() {
            let n = ordinals.entry(ordinal_key).or_insert(0);
            let cur = *n;
            *n += 1;
            cur
        } else {
            0
        };
        let line = find_decl_line(&lines, &build.kind, build.name.as_deref(), ordinal);
        declarations.push(build_timing(build, &nodes, line));
    }

    let categories =
        summary
            .cumulative
            .iter()
            .fold(BTreeMap::new(), |mut acc: BTreeMap<String, f64>, (label, ms)| {
                *acc.entry(label.clone()).or_insert(0.0) += ms;
                acc
            });

    let mut tactics: Vec<TacticTiming> = summary
        .tactic_runs
        .iter()
        .map(|(name, ms, count)| TacticTiming {
            name: name.strip_prefix(TACTIC_PREFIX).unwrap_or(name).to_string(),
            total_ms: round_ms(*ms),
            count: *count,
        })
        .collect();
    tactics.sort_by(|a, b| b.total_ms.total_cmp(&a.total_ms));

    Analysis {
        declarations,
        categories,
        tactics,
    }
}

fn collect_declarations(nodes: &[Node]) -> Vec<DeclBuild> {
    let mut builds: Vec<DeclBuild> = Vec::new();
    let roots: Vec<usize> = (0..nodes.len()).filter(|&i| nodes[i].parent.is_none()).collect();

    for &r in &roots {
        let node = &nodes[r];
        match node.tag.as_str() {
            "Elab.command" => {
                if let Some(h) = parse_decl_header(&node.text) {
                    builds.push(DeclBuild {
                        kind: h.kind,
                        name: h.name,
                        command: Some(r),
                        ..DeclBuild::default()
                    });
                }
            }
            "Elab.async" => {
                if let Some(name) = node.text.strip_prefix("elaborating proof of ") {
                    let name = name.trim();
                    match builds
                        .iter_mut()
                        .find(|b| b.name.as_deref() == Some(name) && b.proof.is_none())
                    {
                        Some(b) => b.proof = Some(r),
                        None => builds.push(DeclBuild {
                            kind: "decl".to_string(),
                            name: Some(name.to_string()),
                            proof: Some(r),
                            ..DeclBuild::default()
                        }),
                    }
                }
            }
            _ => {}
        }
    }

    // Kernel checks may be nested under an async root (for example `addDeclCore`), so search
    // every node rather than only the roots.
    for (r, node) in nodes.iter().enumerate() {
        if node.tag != "Kernel" {
            continue;
        }
        let Some(list) = node
            .text
            .strip_prefix("typechecking declarations [")
            .and_then(|t| t.strip_suffix(']'))
        else {
            continue;
        };
        for entry in list.split(", ") {
            let base = entry.split("._proof").next().unwrap_or(entry).trim();
            if let Some(b) = builds.iter_mut().find(|b| b.name.as_deref() == Some(base)) {
                b.kernel.push(r);
                break;
            }
        }
    }
    builds
}

fn build_timing(build: &DeclBuild, nodes: &[Node], line: u32) -> DeclTiming {
    let roots: Vec<usize> = build
        .command
        .into_iter()
        .chain(build.proof)
        .chain(build.kernel.iter().copied())
        .collect();
    let elab_ms = build.command.map_or(0.0, |r| nodes[r].total_ms);
    let proof_ms = build.proof.map_or(0.0, |r| nodes[r].total_ms);
    let kernel_ms = build.kernel.iter().fold(0.0, |acc, &r| acc + nodes[r].total_ms);

    let mut self_by_label: BTreeMap<String, f64> = BTreeMap::new();
    for &root in &roots {
        for idx in subtree(nodes, root) {
            let node = &nodes[idx];
            if STRUCTURAL_TAGS.contains(&node.tag.as_str()) {
                continue;
            }
            *self_by_label.entry(cost_label(&node.tag)).or_insert(0.0) += node.self_ms(nodes);
        }
    }
    let main = self_by_label
        .iter()
        .filter(|(_, ms)| **ms > 0.0)
        .max_by(|a, b| a.1.total_cmp(b.1));

    DeclTiming {
        kind: build.kind.clone(),
        name: build.name.clone(),
        line,
        total_ms: round_ms(elab_ms + proof_ms + kernel_ms),
        elab_ms: round_ms(elab_ms),
        proof_ms: round_ms(proof_ms),
        kernel_ms: round_ms(kernel_ms),
        main_cost: main.map(|(label, _)| label.clone()),
        main_cost_ms: round_ms(main.map_or(0.0, |(_, ms)| *ms)),
    }
}

/// Human-readable cost category for a trace tag.
pub fn cost_label(tag: &str) -> String {
    let lower = tag.to_ascii_lowercase();
    let label = if tag.starts_with("Meta.synthInstance") || tag.starts_with("Meta.synthPending") {
        "typeclass resolution"
    } else if tag.starts_with("Meta.isDefEq") || tag.starts_with("Meta.isLevelDefEq") {
        "unification (isDefEq)"
    } else if tag.starts_with("Meta.whnf") || tag.starts_with("Meta.reduce") {
        "reduction (whnf)"
    } else if tag == "Kernel" {
        "kernel type checking"
    } else if tag.starts_with("Compiler") {
        "code generation"
    } else if tag.starts_with("Elab.lint") || tag.starts_with("Linter") {
        "linting"
    } else if lower.contains("simp") {
        "simp/search automation"
    } else if lower.contains("tactic") || lower.contains("omega") || lower.contains("decide") {
        "tactic execution"
    } else if tag.starts_with("Elab") {
        "elaboration"
    } else {
        return tag.to_string();
    };
    label.to_string()
}

/// Best-effort line lookup (1-based) of a declaration in the original source. Named declarations
/// are matched by name. Anonymous ones (`example`, `instance : ...`) are matched by ordinal among
/// anonymous declarations of the same keyword. Returns 0 when no line matches.
fn find_decl_line(lines: &[&str], kind: &str, name: Option<&str>, ordinal: usize) -> u32 {
    let mut seen = 0usize;
    for (i, line) in lines.iter().enumerate() {
        let Some(h) = parse_decl_header(line) else {
            continue;
        };
        if kind != "decl" && h.kind != kind {
            continue;
        }
        match name {
            Some(n) => {
                if h.name.as_deref() == Some(n) {
                    return (i + 1) as u32;
                }
            }
            None => {
                if h.name.is_none() {
                    if seen == ordinal {
                        return (i + 1) as u32;
                    }
                    seen += 1;
                }
            }
        }
    }
    0
}

/// Round a millisecond value to 0.1 microseconds, which is finer than Lean's output precision.
/// Keeps JSON reports stable and free of accumulated floating-point noise.
pub fn round_ms(ms: f64) -> f64 {
    let rounded = (ms * 10_000.0).round() / 10_000.0;
    if rounded == 0.0 {
        0.0
    } else {
        rounded
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRACE: &str = include_str!("../tests/fixtures/trace_sample.txt");
    const PROFILER: &str = include_str!("../tests/fixtures/profiler_sample.txt");
    const SOURCE: &str = include_str!("../tests/fixtures/sample.lean");

    fn combined() -> String {
        format!("{TRACE}\n{PROFILER}")
    }

    #[test]
    fn declarations_are_attributed_from_real_output() {
        let a = analyze(&combined(), SOURCE);
        let names: Vec<(&str, Option<&str>)> = a
            .declarations
            .iter()
            .map(|d| (d.kind.as_str(), d.name.as_deref()))
            .collect();
        assert!(names.contains(&("theorem", Some("sumlemma"))), "{names:?}");
        assert!(names.contains(&("example", None)), "{names:?}");
        // `heavy` has no command root above the threshold, only its async proof and kernel check.
        let heavy = a
            .declarations
            .iter()
            .find(|d| d.name.as_deref() == Some("heavy"))
            .expect("heavy from async proof");
        assert_eq!(heavy.kind, "decl");
        assert!((heavy.proof_ms - 3.062).abs() < 1e-6);
        assert!((heavy.kernel_ms - 5.240).abs() < 1e-6);
        assert!((heavy.total_ms - (3.062 + 5.240)).abs() < 1e-6);
        assert_eq!(heavy.line, 11);
    }

    #[test]
    fn line_numbers_point_at_source() {
        let a = analyze(&combined(), SOURCE);
        let by_name = |n: &str| {
            a.declarations
                .iter()
                .find(|d| d.name.as_deref() == Some(n))
                .unwrap()
                .line
        };
        assert_eq!(by_name("sumlemma"), 6);
        let class = a
            .declarations
            .iter()
            .find(|d| d.kind == "class")
            .expect("class decl");
        assert_eq!(class.line, 1);
        let example = a.declarations.iter().find(|d| d.kind == "example").unwrap();
        assert_eq!(example.line, 14);
    }

    #[test]
    fn categories_and_tactics_are_collected() {
        let a = analyze(&combined(), SOURCE);
        assert_eq!(a.categories.get("elaboration").copied(), Some(9.64));
        let omega = a.tactics.iter().find(|t| t.name == "omega").expect("omega");
        assert_eq!(omega.count, 1);
        assert!((omega.total_ms - 2.34).abs() < 1e-9);
    }

    #[test]
    fn main_cost_uses_self_time_by_tag() {
        // Structure: root A (10ms) with child B (9ms) which is a Meta.isDefEq node of 8ms.
        let text = "[Elab.command] [0.010] ✅️ theorem m (a : Nat) : a = a := rfl\n  [Meta.isDefEq] [0.008] ✅️ a =?= a\n";
        let a = analyze(text, "theorem m (a : Nat) : a = a := rfl\n");
        let d = &a.declarations[0];
        assert_eq!(d.main_cost.as_deref(), Some("unification (isDefEq)"));
        assert!((d.main_cost_ms - 8.0).abs() < 1e-9);
        assert!((d.elab_ms - 10.0).abs() < 1e-9);
    }

    #[test]
    fn cost_labels() {
        assert_eq!(cost_label("Meta.synthInstance"), "typeclass resolution");
        assert_eq!(cost_label("Meta.whnf"), "reduction (whnf)");
        assert_eq!(cost_label("Kernel"), "kernel type checking");
        assert_eq!(cost_label("Elab.app.args"), "elaboration");
        assert_eq!(cost_label("Weird.tag"), "Weird.tag");
    }
}
