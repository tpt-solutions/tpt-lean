// SPDX-License-Identifier: MIT OR Apache-2.0
//! Runs the sidecar's `goals` mode on real source files. Skipped (with a message) when the sidecar
//! has not been built for the active Lean version (`cd lean/tpt-lean-sidecar && lake -R build`).

use std::path::{Path, PathBuf};
use tpt_lean_adapter::sidecar::{run_sidecar_goals, run_sidecar_goals_report, GoalSite, GoalTarget};
use tpt_lean_adapter::{discover_toolchain, lean_policy, lean_sysroot};
use tpt_lean_core::policy::{CancelToken, ProcessPolicy};
use tpt_lean_core::ErrorCode;

fn sidecar_exe() -> Option<PathBuf> {
    let tc = discover_toolchain();
    let (version, _) = tpt_lean_adapter::compat::installed(&tc, &lean_policy(60), &CancelToken::new());
    let version = version?;
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../lean/tpt-lean-sidecar/.lake");
    let dir = if version == "4.36.0" {
        base.join("build")
    } else {
        base.join(format!("build-v{version}"))
    };
    let exe = if cfg!(windows) {
        "tpt_sidecar.exe"
    } else {
        "tpt_sidecar"
    };
    let path = dir.join("bin").join(exe);
    path.is_file().then_some(path)
}

fn sysroot() -> Option<PathBuf> {
    let toolchain = discover_toolchain();
    toolchain.lean.as_ref()?;
    lean_sysroot(&toolchain, &lean_policy(60), &CancelToken::new()).ok()
}

fn policy() -> ProcessPolicy {
    ProcessPolicy {
        timeout: std::time::Duration::from_secs(300),
        ..ProcessPolicy::default()
    }
}

/// Run `body` with the sidecar and the toolchain available; skip otherwise.
fn with_sidecar(body: impl FnOnce(&Path, &Path)) {
    let Some(exe) = sidecar_exe() else {
        eprintln!("skipped: sidecar not built (run `lake -R build` in lean/tpt-lean-sidecar)");
        return;
    };
    let Some(root) = sysroot() else {
        eprintln!("skipped: lean not installed");
        return;
    };
    body(&exe, &root);
}

fn temp_project(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tpt-sidecar-goals-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("Goal.lean"),
        "theorem t (a b : Nat) : a + b = b + a := by\n  skip\n",
    )
    .unwrap();
    dir
}

#[test]
fn goals_before_the_tactic_at_a_position() {
    with_sidecar(|exe, root| {
        let dir = temp_project("goals");
        let goals = run_sidecar_goals(
            exe,
            root,
            &GoalSite {
                file: Path::new("Goal.lean"),
                line: 2,
                column: 3,
            },
            Some(&dir),
            None,
            &policy(),
            &CancelToken::new(),
        )
        .expect("goals");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(goals, vec!["a b : Nat\n⊢ a + b = b + a".to_string()]);
    });
}

#[test]
fn no_tactic_goals_outside_a_tactic_block() {
    with_sidecar(|exe, root| {
        let dir = temp_project("outside");
        let report = run_sidecar_goals_report(
            exe,
            root,
            &GoalSite {
                file: Path::new("Goal.lean"),
                line: 1,
                column: 9,
            },
            Some(&dir),
            None,
            &policy(),
            &CancelToken::new(),
        )
        .expect("goals");
        let _ = std::fs::remove_dir_all(&dir);
        // Schema v2: outside a tactic the position may still be a term or a declaration name.
        assert_ne!(report.target, GoalTarget::Tactic, "{:?}", report.goals);
    });
}

#[test]
fn a_missing_file_is_an_io_error() {
    with_sidecar(|exe, root| {
        let dir = temp_project("missing");
        let err = run_sidecar_goals(
            exe,
            root,
            &GoalSite {
                file: Path::new("Missing.lean"),
                line: 1,
                column: 1,
            },
            Some(&dir),
            None,
            &policy(),
            &CancelToken::new(),
        )
        .expect_err("missing file");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(err.code(), ErrorCode::Io, "{err}");
    });
}
