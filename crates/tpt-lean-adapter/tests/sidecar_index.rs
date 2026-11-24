// SPDX-License-Identifier: MIT OR Apache-2.0
//! End to end: a small Lake project is built, the sidecar describes its modules, and the results
//! land in the index. Skipped (with a message) when the sidecar is not built or Lean is missing.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tpt_lean_adapter::sidecar::index_sidecar;
use tpt_lean_adapter::{discover_toolchain, Toolchain};
use tpt_lean_core::policy::{CancelToken, ProcessPolicy};
use tpt_lean_index::{Index, ScanRoot};

struct TempProject(PathBuf);

impl TempProject {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("tpt-sidecar-e2e-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn sidecar_exe() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../lean/tpt-lean-sidecar/.lake/build/bin");
    let exe = if cfg!(windows) {
        "tpt_sidecar.exe"
    } else {
        "tpt_sidecar"
    };
    let path = root.join(exe);
    path.is_file().then_some(path)
}

/// The installed toolchain, or `None` when Lean or Lake is missing.
fn toolchain() -> Option<Toolchain> {
    let tc = discover_toolchain();
    (tc.lean.is_some() && tc.lake.is_some()).then_some(tc)
}

fn policy() -> ProcessPolicy {
    ProcessPolicy {
        timeout: Duration::from_secs(600),
        ..ProcessPolicy::default()
    }
}

#[test]
fn sidecar_results_are_indexed_for_built_modules() {
    let Some(exe) = sidecar_exe() else {
        eprintln!("skipped: sidecar not built (run `lake build` in lean/tpt-lean-sidecar)");
        return;
    };
    let Some(tc) = toolchain() else {
        eprintln!("skipped: lean/lake not installed");
        return;
    };
    let tmp = TempProject::new();
    let dir = &tmp.0;
    let version = tpt_lean_adapter::parse_version(&String::from_utf8_lossy(
        &Command::new(tc.lean.as_ref().unwrap())
            .arg("--version")
            .output()
            .unwrap()
            .stdout,
    ))
    .expect("lean version");
    std::fs::write(
        dir.join("lean-toolchain"),
        format!("leanprover/lean4:v{version}\n"),
    )
    .unwrap();
    std::fs::write(
        dir.join("lakefile.lean"),
        "import Lake\nopen Lake DSL\n\npackage demo\n\n@[default_target]\nlean_lib Demo\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("Demo.lean"),
        "def greeting : String := \"hi\"\n\n/-- The answer. -/\ntheorem answer_ok : 1 = 1 := rfl\n",
    )
    .unwrap();

    let build = Command::new(tc.lake.as_ref().unwrap())
        .arg("build")
        .current_dir(dir)
        .output()
        .expect("lake build starts");
    assert!(
        build.status.success(),
        "lake build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let mut index = Index::open_in_memory().unwrap();
    let roots = vec![ScanRoot {
        package: "demo".into(),
        path: dir.clone(),
    }];
    index.refresh(&roots).unwrap();
    let cancel = CancelToken::new();
    let stats = index_sidecar(&mut index, dir, &exe, &tc, &policy(), &cancel).unwrap();
    assert_eq!(stats.stored, vec!["Demo".to_string()], "{stats:?}");
    assert!(stats.failed.is_empty(), "{stats:?}");

    let greeting = index.declarations_named("greeting", 5).unwrap();
    assert_eq!(greeting.len(), 1, "{greeting:?}");
    assert_eq!(greeting[0].kind, "definition");
    assert_eq!(greeting[0].module, "Demo");
    let answer = index.declarations_named("answer_ok", 5).unwrap();
    assert_eq!(answer[0].kind, "theorem");
    assert_eq!(answer[0].docstring.as_deref(), Some("The answer."));

    // A second run finds nothing stale.
    let again = index_sidecar(&mut index, dir, &exe, &tc, &policy(), &cancel).unwrap();
    assert!(again.stored.is_empty() && again.failed.is_empty(), "{again:?}");

    // A module that was never built is reported, not silently skipped.
    std::fs::write(dir.join("Extra.lean"), "def extra := 1\n").unwrap();
    index.refresh(&roots).unwrap();
    let third = index_sidecar(&mut index, dir, &exe, &tc, &policy(), &cancel).unwrap();
    assert!(third.stored.is_empty(), "{third:?}");
    assert_eq!(third.failed.len(), 1, "{third:?}");
    assert_eq!(third.failed[0].0, "Extra");
}
