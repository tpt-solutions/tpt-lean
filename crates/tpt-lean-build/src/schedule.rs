// SPDX-License-Identifier: MIT OR Apache-2.0
//! Build plan: project modules grouped into waves by import depth.
//!
//! Wave 0 holds modules that import no other project module. Each later wave holds modules whose
//! project imports all lie in earlier waves, so the modules of one wave are independent and can
//! be built at the same time. The number of waves is the critical path; the widest wave bounds
//! the parallelism the import graph allows.
//!
//! The plan is advisory. Lake builds the modules itself, in one process, and runs independent
//! jobs concurrently on its own scheduler. Starting one Lake process per module is much slower
//! than that (see docs/build.md), and concurrent Lake processes sharing one workspace are not
//! known to be safe, so tpt-lean does not run the waves in parallel.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use tpt_lean_core::{Error, ErrorCode, Result};

use crate::project::{ProjectFile, ProjectScan};

/// The waves of a project's module graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Plan {
    /// Number of project modules.
    pub modules: usize,
    /// Modules per wave, each wave sorted by name.
    pub waves: Vec<Vec<String>>,
    /// Number of waves: the longest chain of project imports, counted in modules.
    pub critical_path: usize,
    /// Size of the largest wave.
    pub widest_wave: usize,
}

/// Group the project modules of `scan` into waves. Imports of modules outside the project are
/// ignored. An import cycle is an error, because Lean cannot elaborate it.
pub fn plan(scan: &ProjectScan) -> Result<Plan> {
    // The lakefile configures the build; it is not a module of the project.
    let files: Vec<&ProjectFile> = scan
        .files
        .iter()
        .filter(|f| f.rel_path != "lakefile.lean")
        .collect();
    let mut deps: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for file in &files {
        deps.entry(file.module.as_str()).or_default();
    }
    for file in &files {
        for import in &file.imports {
            if import != &file.module && deps.contains_key(import.as_str()) {
                deps.get_mut(file.module.as_str())
                    .expect("inserted above")
                    .insert(import.as_str());
            }
        }
    }

    let mut done: BTreeSet<&str> = BTreeSet::new();
    let mut waves: Vec<Vec<String>> = Vec::new();
    while done.len() < deps.len() {
        let wave: Vec<&str> = deps
            .iter()
            .filter(|(m, d)| !done.contains(*m) && d.iter().all(|x| done.contains(x)))
            .map(|(m, _)| *m)
            .collect();
        if wave.is_empty() {
            let stuck: Vec<&str> = deps
                .keys()
                .copied()
                .filter(|m| !done.contains(m))
                .collect();
            return Err(Error::new(
                ErrorCode::ConfigInvalid,
                format!("import cycle among modules: {}", stuck.join(", ")),
            ));
        }
        done.extend(wave.iter().copied());
        waves.push(wave.into_iter().map(str::to_string).collect());
    }

    Ok(Plan {
        modules: deps.len(),
        critical_path: waves.len(),
        widest_wave: waves.iter().map(Vec::len).max().unwrap_or(0),
        waves,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::ProjectFile;
    use std::path::PathBuf;

    fn file(module: &str, imports: &[&str]) -> ProjectFile {
        ProjectFile {
            rel_path: format!("{}.lean", module.replace('.', "/")),
            module: module.to_string(),
            sha256: String::new(),
            imports: imports.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn scan(files: Vec<ProjectFile>) -> ProjectScan {
        ProjectScan {
            root: PathBuf::from("."),
            files,
            lakefile_hash: "none".into(),
            manifest_hash: "none".into(),
            packages: Vec::new(),
        }
    }

    #[test]
    fn diamond_has_three_waves_and_a_wide_middle() {
        let p = plan(&scan(vec![
            file("Top", &["Left", "Right", "Std.Data"]),
            file("Left", &["Base"]),
            file("Right", &["Base"]),
            file("Base", &[]),
        ]))
        .unwrap();
        assert_eq!(p.modules, 4);
        assert_eq!(p.waves, vec![vec!["Base"], vec!["Left", "Right"], vec!["Top"]]);
        assert_eq!(p.critical_path, 3);
        assert_eq!(p.widest_wave, 2);
    }

    #[test]
    fn independent_modules_share_one_wave() {
        let p = plan(&scan(vec![file("A", &[]), file("B", &[]), file("C", &[])])).unwrap();
        assert_eq!(p.waves.len(), 1);
        assert_eq!(p.widest_wave, 3);
    }

    #[test]
    fn lakefile_is_not_a_module() {
        let mut lakefile = file("lakefile", &["Lake"]);
        lakefile.rel_path = "lakefile.lean".into();
        let p = plan(&scan(vec![file("Main", &[]), lakefile])).unwrap();
        assert_eq!(p.waves, vec![vec!["Main"]]);
    }

    #[test]
    fn empty_project_has_no_waves() {
        let p = plan(&scan(Vec::new())).unwrap();
        assert!(p.waves.is_empty());
        assert_eq!(p.critical_path, 0);
        assert_eq!(p.widest_wave, 0);
    }

    #[test]
    fn import_cycle_is_an_error() {
        let err = plan(&scan(vec![file("A", &["B"]), file("B", &["A"]), file("C", &[])])).unwrap_err();
        assert_eq!(err.code(), ErrorCode::ConfigInvalid);
        assert!(err.message().contains("A, B"), "{}", err.message());
    }
}
