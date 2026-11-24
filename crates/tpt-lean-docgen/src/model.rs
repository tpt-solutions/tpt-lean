// SPDX-License-Identifier: MIT OR Apache-2.0
//! Collects documentation facts from Lean sources through `tpt-lean-syntax`.

use serde::Serialize;
use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use tpt_lean_core::{Error, ErrorCode, Result};
use tpt_lean_index::{collect_lean_files, default_scan_roots};
use tpt_lean_syntax::parse;

/// Package label used for the project itself (see `tpt_lean_index::default_scan_roots`).
pub const PROJECT_PACKAGE: &str = "project";

/// Maximum characters of a signature kept in the model.
const SIGNATURE_LIMIT: usize = 600;

/// Documentation facts for a project and, optionally, its packages.
#[derive(Debug, Clone, Serialize)]
pub struct Model {
    pub project: String,
    pub packages: Vec<String>,
    pub modules: Vec<ModuleInfo>,
    pub declarations: Vec<DeclInfo>,
    pub imports: Vec<ImportEdge>,
    /// Every namespace that holds a declaration, plus all of its dotted prefixes, sorted.
    pub namespaces: Vec<String>,
}

/// One Lean source file.
#[derive(Debug, Clone, Serialize)]
pub struct ModuleInfo {
    /// Module name derived from the path relative to the package root (`Foo/Bar.lean` -> `Foo.Bar`).
    pub name: String,
    pub package: String,
    /// Path relative to the package root, with `/` separators.
    pub file: String,
    pub declarations: usize,
    pub imports: Vec<String>,
}

/// One top-level declaration.
#[derive(Debug, Clone, Serialize)]
pub struct DeclInfo {
    pub name: String,
    pub short_name: Option<String>,
    pub kind: String,
    pub namespace: String,
    /// Index into `Model::modules`.
    pub module: usize,
    /// 1-based line of the declaration keyword.
    pub line: usize,
    pub docstring: Option<String>,
    pub is_private: bool,
    /// Declaration text up to the first `:=`, truncated.
    pub signature: String,
}

/// An `import` edge from a module to another module name.
#[derive(Debug, Clone, Serialize)]
pub struct ImportEdge {
    /// Index into `Model::modules`.
    pub from: usize,
    pub to: String,
    /// Index into `Model::modules` when the target is one of the documented modules.
    pub resolved: Option<usize>,
}

/// A model plus the raw source text of each module, indexed like `Model::modules`.
pub struct Collected {
    pub model: Model,
    pub sources: Vec<String>,
}

/// Serialize a model as pretty JSON.
pub fn to_json(model: &Model) -> Result<String> {
    serde_json::to_string_pretty(model)
        .map_err(|e| Error::new(ErrorCode::Internal, format!("json export failed: {e}")))
}

/// Walk the project (and its `.lake/packages` when `include_deps`) and extract documentation facts.
pub fn collect(project_root: &Path, include_deps: bool) -> Result<Collected> {
    let roots = default_scan_roots(project_root, include_deps);
    let mut modules = Vec::new();
    let mut sources = Vec::new();
    let mut declarations = Vec::new();
    let mut raw_imports: Vec<(usize, String)> = Vec::new();

    for root in &roots {
        for (rel, abs) in collect_lean_files(&root.path)? {
            let src = std::fs::read_to_string(&abs).map_err(|e| Error::from(e).with_path(&abs))?;
            let parsed = parse(&src);
            let idx = modules.len();
            for imp in &parsed.imports {
                raw_imports.push((idx, imp.module.clone()));
            }
            for sym in &parsed.symbols {
                declarations.push(DeclInfo {
                    name: sym.name.clone(),
                    short_name: sym.short_name.clone(),
                    kind: sym.kind.clone(),
                    namespace: sym.namespace.clone(),
                    module: idx,
                    line: sym.line,
                    docstring: sym.docstring.clone(),
                    is_private: sym.is_private,
                    signature: signature(sym.range.text(&src)),
                });
            }
            modules.push(ModuleInfo {
                name: module_name(&rel),
                package: root.package.clone(),
                file: rel,
                declarations: parsed.symbols.len(),
                imports: parsed.imports.iter().map(|i| i.module.clone()).collect(),
            });
            sources.push(src);
        }
    }

    // First module with a given name wins; the project is scanned before its packages.
    let mut by_name: HashMap<&str, usize> = HashMap::new();
    for (i, m) in modules.iter().enumerate() {
        by_name.entry(m.name.as_str()).or_insert(i);
    }
    let imports = raw_imports
        .iter()
        .map(|(from, to)| ImportEdge {
            from: *from,
            to: to.clone(),
            resolved: by_name.get(to.as_str()).copied(),
        })
        .collect();

    let mut namespaces = BTreeSet::new();
    for d in &declarations {
        let mut acc = String::new();
        for seg in d.namespace.split('.').filter(|s| !s.is_empty()) {
            if !acc.is_empty() {
                acc.push('.');
            }
            acc.push_str(seg);
            namespaces.insert(acc.clone());
        }
    }

    let project = project_root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".to_string());
    let model = Model {
        project,
        packages: roots.iter().map(|r| r.package.clone()).collect(),
        modules,
        declarations,
        imports,
        namespaces: namespaces.into_iter().collect(),
    };
    Ok(Collected { model, sources })
}

/// `Foo/Bar.lean` -> `Foo.Bar`.
fn module_name(rel: &str) -> String {
    rel.strip_suffix(".lean").unwrap_or(rel).replace('/', ".")
}

/// Declaration header: the text before the first `:=`, trimmed and truncated.
fn signature(text: &str) -> String {
    let head = match text.find(":=") {
        Some(i) => &text[..i],
        None => text,
    };
    head.chars()
        .take(SIGNATURE_LIMIT)
        .collect::<String>()
        .trim_end()
        .to_string()
}
