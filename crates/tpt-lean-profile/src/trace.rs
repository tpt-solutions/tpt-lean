// SPDX-License-Identifier: MIT OR Apache-2.0
//! Parsing of Lean's `trace.profiler` tree output and of the plain `profiler` summary lines.
//!
//! With `set_option trace.profiler true`, Lean prints one tree node per line:
//! `[Tag] [seconds] <status> <text>`, with children indented two spaces deeper than their
//! parent. Text may continue on following lines; only the first line of a node is kept.
//! With `set_option profiler true`, Lean prints `<label> took <time>` lines and a
//! `cumulative profiling times:` block of tab-indented `<label> <time>` entries.

/// One node of the trace tree.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub tag: String,
    /// Wall time of the node including children, in milliseconds.
    pub total_ms: f64,
    /// First line of the node text, with the status marker removed.
    pub text: String,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
}

impl Node {
    /// Time spent in this node excluding its children (clamped at zero).
    pub fn self_ms(&self, nodes: &[Node]) -> f64 {
        let child: f64 = self.children.iter().map(|&c| nodes[c].total_ms).sum();
        (self.total_ms - child).max(0.0)
    }
}

/// Parse all trace nodes in `text` into a forest, returned as a flat arena. Root nodes have
/// `parent == None`. Lines that are not node headers are ignored.
pub fn parse_trace(text: &str) -> Vec<Node> {
    let mut nodes: Vec<Node> = Vec::new();
    // Stack of (indent, index) for the currently open ancestors.
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for line in text.lines() {
        let body = line.trim_start();
        let indent = line.len() - body.len();
        let Some((tag, total_ms, text)) = parse_node_header(body) else {
            continue;
        };
        while stack.last().is_some_and(|&(i, _)| i >= indent) {
            stack.pop();
        }
        let parent = stack.last().map(|&(_, idx)| idx);
        let idx = nodes.len();
        nodes.push(Node {
            tag,
            total_ms,
            text,
            parent,
            children: Vec::new(),
        });
        if let Some(p) = parent {
            nodes[p].children.push(idx);
        }
        stack.push((indent, idx));
    }
    nodes
}

/// Indices of `root` and all of its descendants.
pub fn subtree(nodes: &[Node], root: usize) -> Vec<usize> {
    let mut out = vec![root];
    let mut i = 0;
    while i < out.len() {
        let cur = out[i];
        out.extend(nodes[cur].children.iter().copied());
        i += 1;
    }
    out
}

/// Parse `[Tag] [secs] text`. Returns `(tag, milliseconds, text)`.
fn parse_node_header(body: &str) -> Option<(String, f64, String)> {
    let rest = body.strip_prefix('[')?;
    let close = rest.find("] [")?;
    let tag = &rest[..close];
    if tag.is_empty() || tag.contains(char::is_whitespace) {
        return None;
    }
    let rest = &rest[close + 3..];
    let end = rest.find(']')?;
    let secs: f64 = rest[..end].trim().parse().ok()?;
    let text = strip_status(rest[end + 1..].trim_start());
    Some((tag.to_string(), secs * 1000.0, text.to_string()))
}

/// Remove a leading status marker such as a check mark, which contains no ASCII letters or digits.
fn strip_status(text: &str) -> &str {
    let is_marker = |s: &str| !s.is_empty() && !s.chars().any(|c| c.is_ascii_alphanumeric());
    match text.split_once(char::is_whitespace) {
        Some((first, rest)) if is_marker(first) => rest.trim_start(),
        None if is_marker(text) => "",
        _ => text,
    }
}

/// Parse a Lean duration such as `12.9ms`, `840µs`, `3ns` or `1.5s` into milliseconds.
/// Only `ms` has been observed in real output; the other units are accepted defensively.
pub fn parse_duration_ms(text: &str) -> Option<f64> {
    let text = text.trim();
    let split = text
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(text.len());
    let value: f64 = text[..split].parse().ok()?;
    match &text[split..] {
        "ms" => Some(value),
        "s" => Some(value * 1000.0),
        "µs" | "us" => Some(value / 1000.0),
        "ns" => Some(value / 1_000_000.0),
        _ => None,
    }
}

/// Plain profiler output: the cumulative block and the per-tactic `took` lines.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Summary {
    /// Label and total milliseconds from the `cumulative profiling times:` block.
    pub cumulative: Vec<(String, f64)>,
    /// `tactic execution of X took T` lines: (tactic name, total ms, run count).
    pub tactic_runs: Vec<(String, f64, usize)>,
}

pub fn parse_summary(text: &str) -> Summary {
    let mut summary = Summary::default();
    let mut in_cumulative = false;
    for line in text.lines() {
        if line.trim() == "cumulative profiling times:" {
            in_cumulative = true;
            continue;
        }
        if in_cumulative {
            if let Some(entry) = line.strip_prefix('\t') {
                if let Some((label, value)) = entry.trim().rsplit_once(' ') {
                    if let Some(ms) = parse_duration_ms(value) {
                        summary.cumulative.push((label.trim().to_string(), ms));
                    }
                }
                continue;
            }
            in_cumulative = false;
        }
        if let Some((label, value)) = line.trim().rsplit_once(" took ") {
            let Some(ms) = parse_duration_ms(value) else {
                continue;
            };
            if let Some(name) = label.strip_prefix("tactic execution of ") {
                let name = name.trim().to_string();
                match summary.tactic_runs.iter_mut().find(|(n, _, _)| *n == name) {
                    Some(entry) => {
                        entry.1 += ms;
                        entry.2 += 1;
                    }
                    None => summary.tactic_runs.push((name, ms, 1)),
                }
            }
        }
    }
    summary
}

/// A declaration header recognised in a command's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclHeader {
    pub kind: String,
    pub name: Option<String>,
}

const DECL_KINDS: &[&str] = &[
    "theorem",
    "lemma",
    "def",
    "abbrev",
    "instance",
    "example",
    "structure",
    "class",
    "inductive",
    "opaque",
    "axiom",
];

/// Strip leading attributes and visibility/safety modifiers, then read the declaration keyword
/// and, when present, its name.
pub fn parse_decl_header(text: &str) -> Option<DeclHeader> {
    let mut rest = text.trim_start();
    if let Some(after) = rest.strip_prefix("@[") {
        rest = after.split_once(']').map(|(_, r)| r.trim_start())?;
    }
    let mut tokens = rest.split_whitespace();
    let mut kind_tok = tokens.next()?;
    while matches!(
        kind_tok,
        "private" | "protected" | "noncomputable" | "partial" | "unsafe" | "nonrec"
    ) {
        kind_tok = tokens.next()?;
    }
    if !DECL_KINDS.contains(&kind_tok) {
        return None;
    }
    let name = tokens
        .next()
        .filter(|t| is_name_token(t))
        .map(|t| t.trim_end_matches(|c: char| !is_ident_char(c)).to_string());
    Some(DeclHeader {
        kind: kind_tok.to_string(),
        name,
    })
}

fn is_name_token(tok: &str) -> bool {
    match tok.chars().next() {
        Some(c) => is_ident_char(c) && !c.is_ascii_digit() && tok != "where",
        None => false,
    }
}

/// Characters that may appear inside a Lean identifier.
pub fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '\'' | '.' | '!' | '?')
}

/// Folded stacks (`a;b;c <weight>` per line, the input of flamegraph.pl and inferno). Each
/// node contributes one line whose weight is its self time in microseconds, under the path of
/// ancestors above it. Frames are the node tag and the first words of its text.
pub fn folded_stacks(nodes: &[Node]) -> String {
    let mut out = String::new();
    for node in nodes {
        let self_us = (node.self_ms(nodes) * 1000.0).round() as u64;
        if self_us == 0 {
            continue;
        }
        let mut frames = vec![frame_label(node)];
        let mut cur = node.parent;
        while let Some(p) = cur {
            frames.push(frame_label(&nodes[p]));
            cur = nodes[p].parent;
        }
        frames.reverse();
        out.push_str(&frames.join(";"));
        out.push(' ');
        out.push_str(&self_us.to_string());
        out.push('\n');
    }
    out
}

fn frame_label(node: &Node) -> String {
    let words: Vec<&str> = node.text.split_whitespace().take(6).collect();
    let label = if words.is_empty() {
        node.tag.clone()
    } else {
        format!("{}: {}", node.tag, words.join(" "))
    };
    label
        .chars()
        .map(|c| if matches!(c, ';' | '\n' | '\r') { ' ' } else { c })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRACE: &str = include_str!("../tests/fixtures/trace_sample.txt");
    const PROFILER: &str = include_str!("../tests/fixtures/profiler_sample.txt");

    #[test]
    fn node_header_and_units() {
        let (tag, ms, text) =
            parse_node_header("[Elab.command] [0.002100] ✅️ theorem t1 (a b : Nat) := by").unwrap();
        assert_eq!(tag, "Elab.command");
        assert!((ms - 2.1).abs() < 1e-9);
        assert_eq!(text, "theorem t1 (a b : Nat) := by");
        assert!(parse_node_header("elaboration took 12.9ms").is_none());
        assert_eq!(parse_duration_ms("12.9ms"), Some(12.9));
        assert_eq!(parse_duration_ms("1.5s"), Some(1500.0));
        assert_eq!(parse_duration_ms("840µs"), Some(0.84));
        assert_eq!(parse_duration_ms("12 furlongs"), None);
    }

    #[test]
    fn tree_structure_matches_indentation() {
        let nodes = parse_trace(
            "[A] [0.010] ✅️ root\n  [B] [0.004] ✅️ child\n    [C] [0.001] ✅️ leaf\n  [D] [0.002] ✅️ child2\n[E] [0.001] ✅️ other",
        );
        assert_eq!(nodes.len(), 5);
        assert_eq!(nodes[0].children, vec![1, 3]);
        assert_eq!(nodes[1].children, vec![2]);
        assert_eq!(nodes[4].parent, None);
        assert!((nodes[0].self_ms(&nodes) - 4.0).abs() < 1e-9);
        assert_eq!(subtree(&nodes, 0), vec![0, 1, 3, 2]);
    }

    #[test]
    fn real_trace_sample_parses() {
        let nodes = parse_trace(TRACE);
        let roots: Vec<&str> = nodes
            .iter()
            .filter(|n| n.parent.is_none())
            .map(|n| n.tag.as_str())
            .collect();
        assert!(roots.contains(&"Elab.command"));
        assert!(roots.contains(&"Elab.async"));
        // Kernel checks are nested under the async `addDeclCore` root in real output.
        assert!(nodes.iter().any(|n| n.tag == "Kernel" && n.parent.is_some()));
        assert!(nodes
            .iter()
            .any(|n| n.tag == "Elab.command" && n.text.starts_with("theorem sumlemma")));
        // `heavy` is below the trace threshold for its command, but its async proof is kept.
        assert!(nodes
            .iter()
            .any(|n| n.tag == "Elab.async" && n.text == "elaborating proof of heavy"));
    }

    #[test]
    fn decl_headers() {
        let h = parse_decl_header("theorem t1 (a b : Nat) : a + b = b + a := by").unwrap();
        assert_eq!((h.kind.as_str(), h.name.as_deref()), ("theorem", Some("t1")));
        let h = parse_decl_header("@[simp] private def fib : Nat → Nat").unwrap();
        assert_eq!((h.kind.as_str(), h.name.as_deref()), ("def", Some("fib")));
        let h = parse_decl_header("instance : Shape Nat :=").unwrap();
        assert_eq!((h.kind.as_str(), h.name.as_deref()), ("instance", None));
        let h = parse_decl_header("instance inst1 : Inhabited Nat := ⟨3⟩").unwrap();
        assert_eq!(h.name.as_deref(), Some("inst1"));
        let h = parse_decl_header("example : bigsum = bigsum := rfl").unwrap();
        assert_eq!((h.kind.as_str(), h.name.as_deref()), ("example", None));
        assert!(parse_decl_header("#eval foo").is_none());
        assert!(parse_decl_header("set_option trace.profiler true").is_none());
    }

    #[test]
    fn summary_parses_cumulative_and_tactics() {
        let s = parse_summary(PROFILER);
        assert!(s.cumulative.iter().any(|(l, _)| l == "elaboration"));
        assert!(s.cumulative.iter().any(|(l, _)| l == "type checking"));
        assert!(s.tactic_runs.is_empty());

        // The per-tactic lines are printed on stdout, next to the trace.
        let t = parse_summary(TRACE);
        let (name, ms, count) = t
            .tactic_runs
            .iter()
            .find(|(n, _, _)| n.ends_with("omega"))
            .expect("omega timing present")
            .clone();
        assert_eq!(name, "Lean.Parser.Tactic.omega");
        assert!((ms - 2.34).abs() < 1e-9);
        assert_eq!(count, 1);
    }
}

#[cfg(test)]
mod folded_tests {
    use super::*;

    #[test]
    fn folded_lines_use_self_time_in_microseconds() {
        let nodes = parse_trace(
            "[A] [0.010] ✅️ root\n  [B] [0.004] ✅️ child\n    [C] [0.001] ✅️ leaf\n  [D] [0.002] ✅️ child2\n[E] [0.001] ✅️ other",
        );
        let folded = folded_stacks(&nodes);
        let lines: Vec<&str> = folded.lines().collect();
        assert_eq!(
            lines,
            vec![
                "A: root 4000",
                "A: root;B: child 3000",
                "A: root;B: child;C: leaf 1000",
                "A: root;D: child2 2000",
                "E: other 1000",
            ]
        );
    }

    #[test]
    fn no_trace_means_no_stacks() {
        assert!(folded_stacks(&parse_trace("elaboration took 12ms\n")).is_empty());
    }
}
