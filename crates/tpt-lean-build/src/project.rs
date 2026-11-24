// SPDX-License-Identifier: MIT OR Apache-2.0
//! Project understanding: which Lean files exist, what each one imports, and their hashes.
//!
//! This is a file-level module graph built from `import` lines (via `tpt-lean-syntax`). It does
//! not resolve source directories from the Lake configuration; a module `A.B` maps to `A/B.lean`
//! relative to the project root.

use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use tpt_lean_core::hash::{file_sha256, sha256_hex};
use tpt_lean_core::{Error, ErrorCode, Result};

/// Directory names never scanned for sources.
const SKIP_DIRS: &[&str] = &["build", "target", "node_modules"];

/// One Lean source file of the project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectFile {
    /// Path relative to the project root, with `/` separators.
    pub rel_path: String,
    /// Module name, for example `Demo.Basic`.
    pub module: String,
    pub sha256: String,
    /// Import module names, in source order, without duplicates.
    pub imports: Vec<String>,
}

/// The inputs a cache key is computed from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectScan {
    pub root: PathBuf,
    /// All `.lean` files, sorted by `rel_path`.
    pub files: Vec<ProjectFile>,
    /// Hash of the lakefile(s), or `none` when there is none.
    pub lakefile_hash: String,
    /// Hash of `lake-manifest.json`, or `none`.
    pub manifest_hash: String,
    /// `name@rev` for each package in `lake-manifest.json`, sorted.
    pub packages: Vec<String>,
}

impl ProjectScan {
    /// Map a module name to its scanned file, if the module is part of the project.
    pub fn file_for_module(&self, module: &str) -> Option<&ProjectFile> {
        self.files.iter().find(|f| f.module == module)
    }
}

/// Module name for a relative `.lean` path, for example `Demo/Basic.lean` -> `Demo.Basic`.
pub fn module_name(rel_path: &str) -> String {
    rel_path
        .strip_suffix(".lean")
        .unwrap_or(rel_path)
        .replace(['/', '\\'], ".")
}

/// Scan a Lake project: every `.lean` file (skipping hidden and build directories), the lakefile,
/// and the lock file.
pub fn scan_project(root: &Path) -> Result<ProjectScan> {
    if !root.is_dir() {
        return Err(
            Error::new(ErrorCode::ProjectNotFound, "project root is not a directory").with_path(root),
        );
    }
    let mut paths = Vec::new();
    walk(root, &mut paths)?;
    let mut raw: Vec<(String, PathBuf)> = paths.into_iter().map(|p| (rel_string(root, &p), p)).collect();
    raw.sort_by(|a, b| a.0.cmp(&b.0));

    let mut sources = Vec::with_capacity(raw.len());
    for (rel, path) in &raw {
        let text = std::fs::read_to_string(path).map_err(|e| Error::from(e).with_path(path))?;
        let module = module_name(rel);
        let imports = tpt_lean_syntax::parse(&text)
            .imports
            .into_iter()
            .map(|i| i.module)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        sources.push(ProjectFile {
            rel_path: rel.clone(),
            module,
            sha256: sha256_hex(text.as_bytes()),
            imports,
        });
    }

    let mut lakefile_parts = String::new();
    for name in ["lakefile.lean", "lakefile.toml", "lakefile"] {
        let p = root.join(name);
        if p.is_file() {
            lakefile_parts.push_str(&format!("{name}={}\n", file_sha256(&p)?));
        }
    }
    let lakefile_hash = if lakefile_parts.is_empty() {
        "none".to_string()
    } else {
        sha256_hex(lakefile_parts.as_bytes())
    };

    let manifest_path = root.join("lake-manifest.json");
    let (manifest_hash, packages) = if manifest_path.is_file() {
        let hash = file_sha256(&manifest_path)?;
        let text =
            std::fs::read_to_string(&manifest_path).map_err(|e| Error::from(e).with_path(&manifest_path))?;
        (hash, manifest_packages(&text))
    } else {
        ("none".to_string(), Vec::new())
    };

    Ok(ProjectScan {
        root: root.to_path_buf(),
        files: sources,
        lakefile_hash,
        manifest_hash,
        packages,
    })
}

#[derive(Deserialize)]
struct Manifest {
    #[serde(default)]
    packages: Vec<ManifestPackage>,
}

#[derive(Deserialize)]
struct ManifestPackage {
    name: String,
    #[serde(default)]
    rev: Option<String>,
}

/// `name@rev` entries from a `lake-manifest.json` text. Invalid JSON yields no entries.
pub fn manifest_packages(text: &str) -> Vec<String> {
    let Ok(m) = serde_json::from_str::<Manifest>(text) else {
        return Vec::new();
    };
    let mut out: Vec<String> = m
        .packages
        .into_iter()
        .map(|p| format!("{}@{}", p.name, p.rev.unwrap_or_else(|| "?".into())))
        .collect();
    out.sort();
    out
}

/// Content hash of every input file, keyed by relative path. Used by watch mode to detect edits.
pub type Fingerprint = BTreeMap<String, String>;

/// Compute the [`Fingerprint`] of the project's Lean sources, lakefile, lock file, and toolchain
/// pin. Build outputs under `.lake` are excluded, so a build never changes its own fingerprint.
/// Contents are hashed rather than compared by modification time, so same-size edits made within
/// the filesystem's timestamp resolution are still detected. Cost grows with project size.
pub fn fingerprint(root: &Path) -> Result<Fingerprint> {
    let mut paths = Vec::new();
    walk(root, &mut paths)?;
    for name in [
        "lakefile.lean",
        "lakefile.toml",
        "lakefile",
        "lake-manifest.json",
        "lean-toolchain",
    ] {
        let p = root.join(name);
        if p.is_file() {
            paths.push(p);
        }
    }
    let mut out = Fingerprint::new();
    for p in paths {
        out.insert(rel_string(root, &p), file_sha256(&p)?);
    }
    Ok(out)
}

fn rel_string(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Collect `.lean` files below `dir`, skipping hidden directories and [`SKIP_DIRS`].
fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let entries = std::fs::read_dir(dir).map_err(|e| Error::from(e).with_path(dir))?;
    for entry in entries {
        let entry = entry.map_err(|e| Error::from(e).with_path(dir))?;
        let path = entry.path();
        let ft = entry.file_type().map_err(|e| Error::from(e).with_path(&path))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if ft.is_dir() {
            if name.starts_with('.') || SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            walk(&path, out)?;
        } else if ft.is_file() && name.ends_with(".lean") {
            out.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_names() {
        assert_eq!(module_name("Demo/Basic.lean"), "Demo.Basic");
        assert_eq!(module_name("Main.lean"), "Main");
    }

    #[test]
    fn manifest_packages_sorted() {
        let text = r#"{"version":"1.1.0","packages":[
            {"name":"batteries","rev":"bbb","type":"git"},
            {"name":"aesop","rev":"aaa","type":"git"}],"name":"demo"}"#;
        assert_eq!(
            manifest_packages(text),
            vec!["aesop@aaa".to_string(), "batteries@bbb".to_string()]
        );
        assert!(manifest_packages("not json").is_empty());
    }
}
