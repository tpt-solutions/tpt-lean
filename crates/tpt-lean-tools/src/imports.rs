// SPDX-License-Identifier: MIT OR Apache-2.0
//! Import analysis: unused imports, suggestions from the index, minimize plans, and
//! project-internal import cost.
//!
//! Limits (heuristics, not elaboration): references are matched by identifier text.
//! An import is treated as used when some identifier in the file shares a dotted name form
//! (full name, prefix, or suffix) with a declaration of the imported module. Modules that
//! declare instances, notations, syntax, macros, attributes or tactics are never reported as
//! unused, because they are used without being named. Lean's own elaboration decides the real
//! answer; these results are suggestions.

use crate::{relative_display, Finding};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use tpt_lean_core::dirs::ProjectDirs;
use tpt_lean_core::paths::WorkspaceRoot;
use tpt_lean_core::{Error, ErrorCode, Result};
use tpt_lean_index::{collect_lean_files, fts_query, Index};
use tpt_lean_syntax::lexer::{lex, TokenKind};
use tpt_lean_syntax::{Import, Symbol};

/// Keywords whose declarations are used without being named in the importing file.
const IMPLICIT_KEYWORDS: &[&str] = &[
    "instance",
    "notation",
    "infix",
    "infixl",
    "infixr",
    "prefix",
    "postfix",
    "syntax",
    "macro",
    "macro_rules",
    "elab",
    "elab_rules",
    "attribute",
    "simproc",
    "initialize",
    "declare_syntax_cat",
    "register_simp_attr",
    "tactic",
];

/// Heavy-import thresholds used by [`import_costs`].
pub const HEAVY_TRANSITIVE: usize = 50;
pub const HEAVY_FAN_IN: usize = 20;

/// Module name for a project-relative `.lean` path, e.g. `Foo/Bar.lean` gives `Foo.Bar`.
pub fn module_name(rel: &str) -> Option<String> {
    let stem = rel.strip_suffix(".lean")?;
    if stem.is_empty() {
        return None;
    }
    Some(stem.split('/').collect::<Vec<_>>().join("."))
}

/// Reject module names that could escape a directory when turned into a path.
fn valid_module(module: &str) -> bool {
    !module.is_empty()
        && module
            .split('.')
            .all(|seg| !seg.is_empty() && !seg.chars().any(|c| matches!(c, '/' | '\\' | ':' | '\0')))
}

/// Source file for `module`: the project itself first, then each `.lake/packages/<pkg>`.
fn resolve_module(ws: &WorkspaceRoot, module: &str) -> Option<PathBuf> {
    if !valid_module(module) {
        return None;
    }
    let rel = format!("{}.lean", module.replace('.', "/"));
    let root = ws.path();
    let mut candidates = vec![root.join(&rel)];
    let packages = root.join(".lake").join("packages");
    if let Ok(entries) = std::fs::read_dir(&packages) {
        let mut dirs: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| e.path())
            .collect();
        dirs.sort();
        candidates.extend(dirs.into_iter().map(|d| d.join(&rel)));
    }
    candidates.into_iter().find_map(|c| {
        let text = c.to_string_lossy().into_owned();
        let resolved = ws.resolve(&text).ok()?;
        resolved.is_file().then_some(resolved)
    })
}

/// Every prefix and every suffix of a dotted name, e.g. `A.b.c` gives `A`, `A.b`, `b`, `b.c`, ...
fn add_name_forms(out: &mut BTreeSet<String>, name: &str) {
    let parts: Vec<&str> = name.split('.').filter(|p| !p.is_empty()).collect();
    for i in 0..parts.len() {
        out.insert(parts[..=i].join("."));
        out.insert(parts[i..].join("."));
    }
}

/// Identifier forms used in `src`, ignoring import lines, comments and strings.
fn usage_forms(src: &str, imports: &[Import]) -> BTreeSet<String> {
    let mut forms = BTreeSet::new();
    for tok in lex(src) {
        if tok.kind != TokenKind::Ident {
            continue;
        }
        let start = tok.range.start;
        if imports
            .iter()
            .any(|i| i.range.start <= start && start < i.range.end)
        {
            continue;
        }
        add_name_forms(&mut forms, tok.range.text(src));
    }
    forms
}

fn declared_forms(symbols: &[Symbol]) -> BTreeSet<String> {
    let mut forms = BTreeSet::new();
    for s in symbols {
        add_name_forms(&mut forms, &s.name);
        if let Some(short) = &s.short_name {
            add_name_forms(&mut forms, short);
        }
    }
    forms
}

/// True when the source contains declarations that are used without being named.
fn may_be_implicit(src: &str) -> bool {
    lex(src).iter().any(|t| {
        matches!(t.kind, TokenKind::Keyword | TokenKind::Ident)
            && IMPLICIT_KEYWORDS.contains(&t.range.text(src))
    }) || src.contains("@[")
}

fn read_source(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|e| Error::from(e).with_path(path))
}

fn finding(code: &'static str, severity: &'static str, file: &str, line: usize, message: String) -> Finding {
    Finding {
        code,
        severity,
        file: file.to_string(),
        line,
        message,
    }
}

/// Imports in `file` whose module declares names that the file never references.
///
/// Findings:
/// - `import_unused` (warning): no declared name of the module appears in the file.
/// - `import_unresolved` (info): the module's source cannot be found under the project or
///   `.lake/packages`, so it was not analysed.
/// - `import_implicit` (info): the module declares instances, notations, tactics or attributes
///   that are used without being named, so it is not flagged.
pub fn unused_imports(project_root: &Path, file: &Path) -> Result<Vec<Finding>> {
    let ws = WorkspaceRoot::new(project_root)?;
    let target = ws.resolve(&file.to_string_lossy())?;
    let rel = relative_display(ws.path(), &target);
    let src = read_source(&target)?;
    let parsed = tpt_lean_syntax::parse(&src);
    let usage = usage_forms(&src, &parsed.imports);

    let mut findings = Vec::new();
    for imp in &parsed.imports {
        let Some(dep) = resolve_module(&ws, &imp.module) else {
            findings.push(finding(
                "import_unresolved",
                "info",
                &rel,
                imp.line,
                format!(
                    "import {}: source not found under the project or .lake/packages; not analysed",
                    imp.module
                ),
            ));
            continue;
        };
        let Ok(dep_src) = read_source(&dep) else {
            findings.push(finding(
                "import_unresolved",
                "info",
                &rel,
                imp.line,
                format!("import {}: source could not be read; not analysed", imp.module),
            ));
            continue;
        };
        let dep_parse = tpt_lean_syntax::parse(&dep_src);
        let declared = declared_forms(&dep_parse.symbols);
        if declared.is_empty() {
            findings.push(finding(
                "import_unresolved",
                "info",
                &rel,
                imp.line,
                format!(
                    "import {}: module declares no named symbols; not analysed",
                    imp.module
                ),
            ));
            continue;
        }
        if may_be_implicit(&dep_src) {
            findings.push(finding(
                "import_implicit",
                "info",
                &rel,
                imp.line,
                format!(
                    "import {}: module declares instances, notations, tactics or attributes; may be used without naming, not flagged",
                    imp.module
                ),
            ));
            continue;
        }
        if usage.is_disjoint(&declared) {
            findings.push(finding(
                "import_unused",
                "warning",
                &rel,
                imp.line,
                format!(
                    "import {}: no declared name of this module is referenced in the file (heuristic; see docs/tools.md)",
                    imp.module
                ),
            ));
        }
    }
    Ok(findings)
}

/// A module suggested for an identifier query, from the local index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImportSuggestion {
    pub module: String,
    pub package: String,
    pub file: String,
    /// Number of matching declarations in this module.
    pub matches: usize,
    /// Up to three matching declaration names.
    pub examples: Vec<String>,
}

/// Modules whose declarations match `query`, using the project index at `.tpt-lean/index.sqlite`.
///
/// Requires an index: run `tpt-lean index refresh` first. Opening the index may migrate its
/// schema. Matches are exact, prefix, or full-text matches over declaration names and docstrings.
pub fn suggest_imports(project_root: &Path, query: &str, limit: usize) -> Result<Vec<ImportSuggestion>> {
    let query = query.trim();
    if query.is_empty() {
        return Err(Error::invalid("imports --suggest needs a non-empty query"));
    }
    let db = ProjectDirs::new(project_root).index_db();
    if !db.is_file() {
        return Err(Error::new(
            ErrorCode::IndexError,
            "no index found; run `tpt-lean index refresh` before `imports --suggest`",
        )
        .with_path(db));
    }
    let index = Index::open(&db)?;
    let mut rows = index.symbols_named(query, 200)?;
    if let Some(fts) = fts_query(query) {
        rows.extend(index.symbols_fts(&fts, 200)?);
    }

    let mut seen = BTreeSet::new();
    let mut by_module: BTreeMap<(String, String, String), (usize, Vec<String>)> = BTreeMap::new();
    for row in rows {
        if !seen.insert((row.package.clone(), row.file.clone(), row.name.clone())) {
            continue;
        }
        let Some(module) = module_name(&row.file) else {
            continue;
        };
        let entry = by_module
            .entry((module, row.package.clone(), row.file.clone()))
            .or_insert((0, Vec::new()));
        entry.0 += 1;
        if entry.1.len() < 3 {
            entry.1.push(row.name.clone());
        }
    }

    let mut out: Vec<ImportSuggestion> = by_module
        .into_iter()
        .map(
            |((module, package, file), (matches, examples))| ImportSuggestion {
                module,
                package,
                file,
                matches,
                examples,
            },
        )
        .collect();
    out.sort_by(|a, b| b.matches.cmp(&a.matches).then_with(|| a.module.cmp(&b.module)));
    out.truncate(limit);
    Ok(out)
}

/// A proposed edit that removes unused imports. Never applied by this crate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MinimizePlan {
    pub file: String,
    /// 1-based lines of imports proposed for removal.
    pub removed_lines: Vec<usize>,
    /// Unified diff (empty when nothing is proposed). Not written to disk.
    pub diff: String,
    /// Always false in this version: no files are written.
    pub applied: bool,
}

/// Propose removing every import reported by [`unused_imports`]. Writes nothing.
///
/// Removing an import can break a file that relied on it for instances or transitive names,
/// so the diff must be checked by building the project.
pub fn minimize_imports(project_root: &Path, file: &Path) -> Result<MinimizePlan> {
    let ws = WorkspaceRoot::new(project_root)?;
    let target = ws.resolve(&file.to_string_lossy())?;
    let rel = relative_display(ws.path(), &target);
    let findings = unused_imports(project_root, file)?;
    let mut removed: Vec<usize> = findings
        .iter()
        .filter(|f| f.code == "import_unused")
        .map(|f| f.line)
        .collect();
    removed.sort_unstable();
    removed.dedup();
    let src = read_source(&target)?;
    let lines: Vec<&str> = src.lines().collect();
    let diff = removal_diff(&rel, &lines, &removed);
    Ok(MinimizePlan {
        file: rel,
        removed_lines: removed,
        diff,
        applied: false,
    })
}

/// Unified diff that deletes the given 1-based lines, with three lines of context.
fn removal_diff(rel: &str, lines: &[&str], removed: &[usize]) -> String {
    if removed.is_empty() {
        return String::new();
    }
    const CONTEXT: usize = 3;
    let is_removed = |idx: usize| removed.contains(&(idx + 1));
    // Hunks as half-open ranges over 0-based line indices.
    let mut hunks: Vec<(usize, usize)> = Vec::new();
    for &line in removed {
        let idx = line - 1;
        let start = idx.saturating_sub(CONTEXT);
        let end = (idx + CONTEXT + 1).min(lines.len());
        match hunks.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => hunks.push((start, end)),
        }
    }
    let mut out = format!("--- a/{rel}\n+++ b/{rel}\n");
    let mut removed_before = 0usize;
    for (start, end) in hunks {
        let removed_here = (start..end).filter(|&i| is_removed(i)).count();
        let old_count = end - start;
        let new_count = old_count - removed_here;
        let old_start = start + 1;
        let new_before = start - removed_before;
        let new_start = if new_count == 0 {
            new_before
        } else {
            new_before + 1
        };
        out.push_str(&format!(
            "@@ -{old_start},{old_count} +{new_start},{new_count} @@\n"
        ));
        for (i, line) in lines.iter().enumerate().take(end).skip(start) {
            out.push(if is_removed(i) { '-' } else { ' ' });
            out.push_str(line);
            out.push('\n');
        }
        removed_before += removed_here;
    }
    out
}

/// Import cost of one project module. Counts are structural, not compile time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImportCost {
    pub module: String,
    pub file: String,
    /// Direct imports written in the file (project and external).
    pub direct_imports: usize,
    /// Direct imports whose module is not a project file. These are not expanded.
    pub external_imports: usize,
    /// Distinct project modules reachable through imports, excluding the module itself.
    pub transitive_project_imports: usize,
    /// Number of project modules that import this module directly.
    pub fan_in: usize,
    /// True when the transitive count or fan-in passes the heavy thresholds.
    pub heavy: bool,
}

type ImportGraph = BTreeMap<String, (String, Vec<String>)>;

/// Structural import cost of every project file, sorted heaviest first.
///
/// This is an estimate of how much of the project a module pulls in. It is not a measurement
/// of compile time: Lean elaboration cost, proof size and external packages (Mathlib, Lean core)
/// are not included. Only the project's own files are scanned; `.lake` is skipped.
pub fn import_costs(project_root: &Path) -> Result<Vec<ImportCost>> {
    let files = collect_lean_files(project_root)?;
    let mut graph: ImportGraph = BTreeMap::new();
    for (rel, path) in files {
        let Some(module) = module_name(&rel) else { continue };
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        let parsed = tpt_lean_syntax::parse(&src);
        let imports = parsed.imports.into_iter().map(|i| i.module).collect();
        graph.insert(module, (rel, imports));
    }

    let mut fan_in: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, imports) in graph.values() {
        let unique: BTreeSet<&String> = imports.iter().filter(|m| graph.contains_key(*m)).collect();
        for m in unique {
            *fan_in.entry(m.as_str()).or_insert(0) += 1;
        }
    }

    let mut out = Vec::with_capacity(graph.len());
    for (module, (file, imports)) in &graph {
        let external = imports.iter().filter(|m| !graph.contains_key(*m)).count();
        let transitive = reachable_count(&graph, module);
        let fi = fan_in.get(module.as_str()).copied().unwrap_or(0);
        out.push(ImportCost {
            module: module.clone(),
            file: file.clone(),
            direct_imports: imports.len(),
            external_imports: external,
            transitive_project_imports: transitive,
            fan_in: fi,
            heavy: transitive >= HEAVY_TRANSITIVE || fi >= HEAVY_FAN_IN,
        });
    }
    out.sort_by(|a, b| {
        b.transitive_project_imports
            .cmp(&a.transitive_project_imports)
            .then(b.fan_in.cmp(&a.fan_in))
            .then_with(|| a.module.cmp(&b.module))
    });
    Ok(out)
}

/// Count of project modules reachable from `start` (not counting `start` itself).
fn reachable_count(graph: &ImportGraph, start: &str) -> usize {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut stack: Vec<&str> = vec![start];
    while let Some(m) = stack.pop() {
        let Some((_, imports)) = graph.get(m) else {
            continue;
        };
        for next in imports {
            if let Some((key, _)) = graph.get_key_value(next.as_str()) {
                if key.as_str() != start && seen.insert(key.as_str()) {
                    stack.push(key.as_str());
                }
            }
        }
    }
    seen.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct TempProject(PathBuf);

    impl TempProject {
        fn new(tag: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let dir =
                std::env::temp_dir().join(format!("tpt-tools-imp-{tag}-{}-{nanos}", std::process::id()));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn write(&self, rel: &str, text: &str) {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, text).unwrap();
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn fixture() -> TempProject {
        let p = TempProject::new("fx");
        p.write("Foo/Bar.lean", "namespace Foo\ndef helper := 1\nend Foo\n");
        p.write("Baz.lean", "def unrelated := 2\n");
        p.write("Main.lean", "import Foo.Bar\nimport Baz\n\n#eval Foo.helper\n");
        p.write("Lonely.lean", "import Foo.Bar\nimport Baz\n\ndef x := 3\n");
        p
    }

    #[test]
    fn flags_only_unreferenced_imports() {
        let p = fixture();
        let used = unused_imports(&p.0, Path::new("Main.lean")).unwrap();
        let lines: Vec<usize> = used
            .iter()
            .filter(|f| f.code == "import_unused")
            .map(|f| f.line)
            .collect();
        assert_eq!(
            lines,
            vec![2],
            "only `import Baz` is unused in Main.lean: {used:?}"
        );

        let lonely = unused_imports(&p.0, Path::new("Lonely.lean")).unwrap();
        let unused: Vec<&Finding> = lonely.iter().filter(|f| f.code == "import_unused").collect();
        assert_eq!(unused.len(), 2, "{lonely:?}");
        assert_eq!(unused[0].line, 1);
        assert_eq!(unused[0].file, "Lonely.lean");
        assert_eq!(unused[1].line, 2);
    }

    #[test]
    fn unresolved_and_implicit_modules_are_not_flagged() {
        let p = fixture();
        p.write("Inst.lean", "instance : Inhabited Nat := ⟨0⟩\n");
        p.write("Use.lean", "import Inst\nimport Mathlib.Data.Nat\n\ndef y := 1\n");
        let f = unused_imports(&p.0, Path::new("Use.lean")).unwrap();
        assert!(f.iter().all(|x| x.code != "import_unused"), "{f:?}");
        assert!(f.iter().any(|x| x.code == "import_implicit"));
        assert!(f
            .iter()
            .any(|x| x.code == "import_unresolved" && x.message.contains("Mathlib")));
    }

    #[test]
    fn rejects_paths_outside_root() {
        let p = fixture();
        assert!(unused_imports(&p.0, Path::new("../escape.lean")).is_err());
        assert!(!valid_module("..") && !valid_module("A/B") && valid_module("A.B"));
    }

    #[test]
    fn minimize_produces_diff_without_writing() {
        let p = fixture();
        let before = fs::read_to_string(p.0.join("Lonely.lean")).unwrap();
        let plan = minimize_imports(&p.0, Path::new("Lonely.lean")).unwrap();
        assert!(!plan.applied);
        assert_eq!(plan.removed_lines, vec![1, 2]);
        assert!(plan.diff.contains("--- a/Lonely.lean"));
        assert!(plan.diff.contains("-import Foo.Bar"));
        assert!(plan.diff.contains("@@ -1,"));
        assert_eq!(fs::read_to_string(p.0.join("Lonely.lean")).unwrap(), before);
    }

    #[test]
    fn diff_hunk_counts_are_consistent() {
        let lines = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"];
        let diff = removal_diff("X.lean", &lines, &[5]);
        assert_eq!(
            diff,
            "--- a/X.lean\n+++ b/X.lean\n@@ -2,7 +2,6 @@\n b\n c\n d\n-e\n f\n g\n h\n"
        );
    }

    #[test]
    fn import_costs_rank_and_count() {
        let p = fixture();
        let costs = import_costs(&p.0).unwrap();
        let main = costs.iter().find(|c| c.module == "Main").unwrap();
        assert_eq!(main.direct_imports, 2);
        assert_eq!(main.transitive_project_imports, 2);
        let bar = costs.iter().find(|c| c.module == "Foo.Bar").unwrap();
        assert_eq!(bar.fan_in, 2);
        assert_eq!(bar.transitive_project_imports, 0);
        assert_eq!(costs[0].transitive_project_imports, 2);
    }

    #[test]
    fn suggest_requires_index() {
        let p = fixture();
        let err = suggest_imports(&p.0, "helper", 5).unwrap_err();
        assert_eq!(err.code(), ErrorCode::IndexError);
        assert!(suggest_imports(&p.0, "  ", 5).is_err());
    }
}
