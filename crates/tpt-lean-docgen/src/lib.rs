// SPDX-License-Identifier: MIT OR Apache-2.0
//! Static documentation generator for Lean projects.
//!
//! Facts come from `tpt-lean-syntax` (declarations, namespaces, docstrings, imports), so no
//! Lean process runs and no network access is needed. The output is a folder of static HTML
//! with a local search index (`search-index.json`), dark mode, source pages with line anchors,
//! a namespace tree, an import graph, and a JSON export (`api.json`).

mod model;
mod render;

use serde::Serialize;
use std::path::{Path, PathBuf};
use tpt_lean_core::Result;

pub use model::{DeclInfo, ImportEdge, Model, ModuleInfo};

/// What `generate` produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Summary {
    pub project: String,
    pub modules: usize,
    pub declarations: usize,
    pub namespaces: usize,
    pub import_edges: usize,
    /// Import edges whose target is not one of the documented modules.
    pub external_imports: usize,
    pub files_written: usize,
    pub out_dir: PathBuf,
}

/// Generate the static site for the Lean project at `project_root` into `out_dir`.
///
/// `include_deps` also documents the packages under `.lake/packages`. Existing files in
/// `out_dir` with the same names are overwritten; nothing else is removed.
pub fn generate(project_root: &Path, out_dir: &Path, include_deps: bool) -> Result<Summary> {
    let collected = model::collect(project_root, include_deps)?;
    let files_written = render::write_site(out_dir, &collected)?;
    let m = &collected.model;
    let external_imports = m.imports.iter().filter(|e| e.resolved.is_none()).count();
    Ok(Summary {
        project: m.project.clone(),
        modules: m.modules.len(),
        declarations: m.declarations.len(),
        namespaces: m.namespaces.len(),
        import_edges: m.imports.len(),
        external_imports,
        files_written,
        out_dir: out_dir.to_path_buf(),
    })
}

/// Export the documentation model as pretty-printed JSON without writing any files.
pub fn export_json(project_root: &Path, include_deps: bool) -> Result<String> {
    let collected = model::collect(project_root, include_deps)?;
    model::to_json(&collected.model)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// A unique temp directory, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let path = std::env::temp_dir().join(format!("tpt-lean-docgen-test-{}-{n}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn write(&self, rel: &str, content: &str) {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, content).unwrap();
        }

        fn read(&self, rel: &str) -> String {
            fs::read_to_string(self.0.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
        }

        fn exists(&self, rel: &str) -> bool {
            self.0.join(rel).is_file()
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const BASIC: &str = "import Sample.Util\n\nnamespace Sample\n\n/-- Adds <b>commutatively</b> & \"quoted\". -/\ntheorem add_comm' (a b : Nat) : a + b = b + a := by\n  omega\n\nprivate def helper := 1\n\nend Sample\n\ntheorem top : True := trivial\n";

    const UTIL: &str =
        "namespace Sample.Util\n\ndef twice (n : Nat) := 2 * n\n\nend Sample.Util\n\nimport Std\n";

    fn sample_project() -> TempDir {
        let t = TempDir::new();
        t.write("Sample/Basic.lean", BASIC);
        t.write("Sample/Util.lean", UTIL);
        t.write("Sample.lean", "import Sample.Basic\n");
        t
    }

    #[test]
    fn generates_site_for_sample_project() {
        let project = sample_project();
        let out = TempDir::new();
        let summary = generate(project.path(), out.path(), false).unwrap();

        assert_eq!(summary.modules, 3);
        assert_eq!(summary.declarations, 4);
        assert_eq!(summary.namespaces, 2);
        assert_eq!(summary.external_imports, 1);
        assert!(summary.files_written > 10);

        for rel in [
            "index.html",
            "namespaces.html",
            "deps.html",
            "style.css",
            "search-index.json",
            "api.json",
        ] {
            assert!(out.exists(rel), "missing {rel}");
        }

        let index = out.read("index.html");
        assert!(index.contains("Sample"));
        assert!(index.contains("search-input"));

        // Theorem page: escaped docstring, signature and a source link with a line anchor.
        let search: serde_json::Value = serde_json::from_str(&out.read("search-index.json")).unwrap();
        let items = search.as_array().unwrap();
        let theorem = items
            .iter()
            .find(|i| i["n"] == "Sample.add_comm'")
            .expect("theorem in search index");
        let href = theorem["u"].as_str().unwrap();
        assert!(href.starts_with("decls/"));
        let page = out.read(href);
        assert!(page.contains("&lt;b&gt;commutatively&lt;/b&gt; &amp; &quot;quoted&quot;."));
        assert!(!page.contains("<b>commutatively</b>"));
        assert!(page.contains("theorem"));
        assert!(page.contains("(a b : Nat) : a + b = b + a"));
        assert!(page.contains("../src/1-Sample_Basic.html#L6"));
        assert!(page.contains("Sample/Basic.lean:6"));

        // Private declarations are kept out of the search index.
        assert!(!items.iter().any(|i| i["n"] == "Sample.helper"));

        // Namespace page lists its declarations and sub-namespaces.
        let ns_page = out.read("ns/0-Sample.html");
        assert!(ns_page.contains("Namespace <code>Sample</code>"));
        assert!(ns_page.contains("Sub-namespaces"));
        assert!(ns_page.contains("Sample.Util"));
        assert!(ns_page.contains("Sample.add_comm&#x27;"));
        let tree = out.read("namespaces.html");
        assert!(tree.contains("Util"));

        // Dependency graph: resolved project edge and an external one.
        let deps = out.read("deps.html");
        assert!(deps.contains("Sample.Util"));
        assert!(deps.contains("(external)"));
        assert!(deps.contains("Imported by:"));

        // Dark mode and self-contained styling.
        let css = out.read("style.css");
        assert!(css.contains("@media (prefers-color-scheme: dark)"));
        assert!(!index.contains("http://") && !index.contains("https://"));
        assert!(!index.contains("<script src"));

        // Source pages escape file contents.
        let src = out.read("src/1-Sample_Basic.html");
        assert!(src.contains("&lt;b&gt;commutatively&lt;/b&gt;"));
        assert!(!src.contains("<b>commutatively</b>"));
        assert!(src.contains("id=\"L6\""));

        // JSON export matches the model that generated api.json.
        let json = export_json(project.path(), false).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let on_disk: serde_json::Value = serde_json::from_str(&out.read("api.json")).unwrap();
        assert_eq!(parsed, on_disk);
        assert_eq!(parsed["modules"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn include_deps_documents_package_sources() {
        let project = sample_project();
        project.write(
            ".lake/packages/dep/Dep.lean",
            "/-- A dependency lemma. -/\ntheorem dep_lemma : True := trivial\n",
        );
        let out = TempDir::new();

        let without = export_json(project.path(), false).unwrap();
        assert!(!without.contains("dep_lemma"));

        let summary = generate(project.path(), out.path(), true).unwrap();
        assert_eq!(summary.modules, 4);
        let json: serde_json::Value =
            serde_json::from_str(&export_json(project.path(), true).unwrap()).unwrap();
        assert!(json["packages"].as_array().unwrap().iter().any(|p| p == "dep"));
        assert!(json["declarations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["name"] == "dep_lemma"));
        assert!(out.exists("api.json"));
    }

    #[test]
    fn empty_project_still_generates_index() {
        let project = TempDir::new();
        let out = TempDir::new();
        let summary = generate(project.path(), &out.path().join("site"), false).unwrap();
        assert_eq!(summary.modules, 0);
        assert!(out.exists("site/index.html"));
        assert!(out.read("site/search-index.json").starts_with('['));
    }
}
