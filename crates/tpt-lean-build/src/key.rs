// SPDX-License-Identifier: MIT OR Apache-2.0
//! Cache key computation (specification section 7.6).
//!
//! The key covers: Lean version and platform, Lake version, package revisions, lakefile hash,
//! lock-file hash, the hash of every project source file, the hashes of imported project modules,
//! the Lake invocation flags, Lean-related environment variables, and the target triple.

use crate::project::{scan_project, ProjectScan};
use std::collections::BTreeSet;
use std::path::Path;
use tpt_lean_adapter::{discover_toolchain, execute, lean_policy, parse_version, Command, Toolchain};
use tpt_lean_core::hash::CacheKey;
use tpt_lean_core::policy::CancelToken;
use tpt_lean_core::{Error, ErrorCode, Result};

/// Versions and platform reported by the installed toolchain.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Tools {
    /// For example `4.36.0`.
    pub lean_version: String,
    /// The full first line of `lake --version`.
    pub lake_version: String,
    /// Platform reported by `lean --version`, for example `x86_64-w64-windows-gnu`.
    pub platform: Option<String>,
}

/// Environment variable prefixes that can change what Lean or Lake produces.
const ENV_PREFIXES: &[&str] = &["LEAN_", "LAKE_", "ELAN_"];

/// Where the probed versions of `tc`'s binaries are memoized: a file named after the binaries'
/// paths, sizes and modification times, so an upgraded toolchain is probed again.
fn tools_memo_path(tc: &Toolchain) -> Option<std::path::PathBuf> {
    fn stamp(p: &Path) -> Option<String> {
        let m = std::fs::metadata(p).ok()?;
        let mtime = m
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos();
        Some(format!("{}|{}|{}", p.display(), m.len(), mtime))
    }
    let lean = stamp(tc.lean.as_deref()?)?;
    let lake = stamp(tc.lake.as_deref()?)?;
    let id = tpt_lean_core::hash::sha256_hex(format!("{lean}\n{lake}").as_bytes());
    Some(
        tpt_lean_core::dirs::global_cache_dir()?
            .join("build")
            .join("tools")
            .join(format!("{id}.json")),
    )
}

/// Versions and platform of the installed toolchain. The result is memoized on disk per binary
/// (see [`tools_memo_path`]), because starting `lake --version` is slow on some platforms; a
/// memo miss starts two processes.
pub fn probe_tools(tc: &Toolchain) -> Result<Tools> {
    if tc.lean.is_none() || tc.lake.is_none() {
        return Err(Error::new(
            ErrorCode::LeanNotFound,
            "lean and lake were not found on PATH or in ~/.elan/bin",
        ));
    }
    let memo = tools_memo_path(tc);
    if let Some(path) = &memo {
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Ok(t) = serde_json::from_str::<Tools>(&text) {
                return Ok(t);
            }
        }
    }
    let tools = probe_tools_uncached(tc)?;
    if let Some(path) = memo {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string(&tools) {
            // Best effort: a failed memo write only costs a later probe.
            let _ = std::fs::write(&path, text);
        }
    }
    Ok(tools)
}

fn probe_tools_uncached(tc: &Toolchain) -> Result<Tools> {
    let token = CancelToken::new();
    let policy = lean_policy(30);
    let lean = execute(tc, &Command::LeanVersion, None, &policy, &token)?;
    let lean_line = lean
        .output
        .stdout
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .to_string();
    let lean_version = parse_version(&lean.output.stdout)
        .ok_or_else(|| Error::new(ErrorCode::LeanNotFound, "could not parse `lean --version`"))?;
    let platform = lean_line
        .split(',')
        .nth(1)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let lake = execute(tc, &Command::LakeVersion, None, &policy, &token)?;
    let lake_version = lake
        .output
        .stdout
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("unknown")
        .trim()
        .to_string();
    Ok(Tools {
        lean_version,
        lake_version,
        platform,
    })
}

/// Build the cache key for a project from its files and the installed toolchain.
pub fn cache_key_for(project_root: &Path, target: Option<&str>) -> Result<CacheKey> {
    let tc = discover_toolchain();
    let tools = probe_tools(&tc)?;
    let scan = scan_project(project_root)?;
    Ok(key_from_parts(&scan, &tools, target))
}

/// Pure part of [`cache_key_for`]: combine an already-scanned project and probed tools.
pub fn key_from_parts(scan: &ProjectScan, tools: &Tools, target: Option<&str>) -> CacheKey {
    let mut source_lines = String::new();
    for f in &scan.files {
        source_lines.push_str(&format!("{}={}\n", f.rel_path, f.sha256));
    }

    // Each imported project module contributes its own content hash; external imports
    // (for example Mathlib or Std) are covered by the package revisions instead.
    let mut import_hashes: BTreeSet<String> = BTreeSet::new();
    for f in &scan.files {
        for m in &f.imports {
            match scan.file_for_module(m) {
                Some(dep) => {
                    import_hashes.insert(format!("internal:{m}={}", dep.sha256));
                }
                None => {
                    import_hashes.insert(format!("external:{m}"));
                }
            }
        }
    }

    let mut env: Vec<(String, String)> = std::env::vars()
        .filter(|(k, _)| {
            let up = k.to_ascii_uppercase();
            ENV_PREFIXES.iter().any(|p| up.starts_with(p))
        })
        .collect();
    env.sort();

    let platform = tools.platform.clone().unwrap_or_default();
    let target_triple = format!("{}-{}:{}", std::env::consts::ARCH, std::env::consts::OS, platform);

    CacheKey {
        lean_version: tools.lean_version.clone(),
        package_versions: scan.packages.join(","),
        lakefile_hash: scan.lakefile_hash.clone(),
        dependency_lock_hash: scan.manifest_hash.clone(),
        source_hash: tpt_lean_core::hash::sha256_hex(source_lines.as_bytes()),
        import_hashes: import_hashes.into_iter().collect(),
        lean_flags: vec![
            format!("lake={}", tools.lake_version),
            format!("target={}", target.unwrap_or("default")),
        ],
        env,
        target_triple,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{module_name, ProjectFile};
    use std::path::PathBuf;

    fn file(rel: &str, sha: &str, imports: &[&str]) -> ProjectFile {
        ProjectFile {
            rel_path: rel.into(),
            module: module_name(rel),
            sha256: sha.into(),
            imports: imports.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn scan(files: Vec<ProjectFile>) -> ProjectScan {
        ProjectScan {
            root: PathBuf::from("p"),
            files,
            lakefile_hash: "lf".into(),
            manifest_hash: "lock".into(),
            packages: vec!["batteries@abc".into()],
        }
    }

    fn tools() -> Tools {
        Tools {
            lean_version: "4.36.0".into(),
            lake_version: "Lake version 5.0.0".into(),
            platform: Some("x86_64-w64-windows-gnu".into()),
        }
    }

    #[test]
    fn key_is_stable_for_identical_inputs() {
        let s = scan(vec![
            file("Main.lean", "h1", &["Demo.Basic"]),
            file("Demo/Basic.lean", "h2", &[]),
        ]);
        let a = key_from_parts(&s, &tools(), None).digest();
        let b = key_from_parts(&s, &tools(), None).digest();
        assert_eq!(a, b);
    }

    #[test]
    fn changing_a_source_changes_the_key() {
        let s1 = scan(vec![
            file("Main.lean", "h1", &[]),
            file("Demo/Basic.lean", "h2", &[]),
        ]);
        let s2 = scan(vec![
            file("Main.lean", "h1", &[]),
            file("Demo/Basic.lean", "h2x", &[]),
        ]);
        assert_ne!(
            key_from_parts(&s1, &tools(), None).digest(),
            key_from_parts(&s2, &tools(), None).digest()
        );
    }

    #[test]
    fn changing_an_imported_module_changes_the_key() {
        let s1 = scan(vec![
            file("Main.lean", "h1", &["Demo.Basic"]),
            file("Demo/Basic.lean", "h2", &[]),
        ]);
        let s2 = scan(vec![
            file("Main.lean", "h1", &["Demo.Basic"]),
            file("Demo/Basic.lean", "h2x", &[]),
        ]);
        let k1 = key_from_parts(&s1, &tools(), None);
        let k2 = key_from_parts(&s2, &tools(), None);
        assert_ne!(k1.import_hashes, k2.import_hashes);
        assert_ne!(k1.digest(), k2.digest());
    }

    #[test]
    fn toolchain_target_and_lock_changes_change_the_key() {
        let s = scan(vec![file("Main.lean", "h1", &[])]);
        let base = key_from_parts(&s, &tools(), None).digest();
        let mut t2 = tools();
        t2.lean_version = "4.37.0".into();
        assert_ne!(key_from_parts(&s, &t2, None).digest(), base);
        assert_ne!(key_from_parts(&s, &tools(), Some("Main")).digest(), base);
        let mut s2 = scan(vec![file("Main.lean", "h1", &[])]);
        s2.manifest_hash = "other".into();
        assert_ne!(key_from_parts(&s2, &tools(), None).digest(), base);
    }

    #[test]
    fn external_imports_are_recorded_by_name() {
        let s = scan(vec![file("Main.lean", "h1", &["Mathlib.Data.Nat"])]);
        let k = key_from_parts(&s, &tools(), None);
        assert_eq!(k.import_hashes, vec!["external:Mathlib.Data.Nat".to_string()]);
    }
}
