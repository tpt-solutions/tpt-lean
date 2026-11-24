// SPDX-License-Identifier: MIT OR Apache-2.0
//! Runs the built Lean sidecar against real modules. Skipped (with a message) when the sidecar
//! has not been built: `cd lean/tpt-lean-sidecar && lake build`.

use std::path::{Path, PathBuf};
use std::time::Duration;
use tpt_lean_adapter::sidecar::{run_sidecar, SidecarReport};
use tpt_lean_adapter::{discover_toolchain, lean_policy, lean_sysroot};
use tpt_lean_core::policy::{CancelToken, ProcessPolicy};
use tpt_lean_core::ErrorCode;

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

fn policy() -> ProcessPolicy {
    ProcessPolicy {
        timeout: Duration::from_secs(300),
        output_limit_bytes: 256 * 1024 * 1024,
        ..ProcessPolicy::default()
    }
}

/// Lean's sysroot, when both the sidecar and a Lean toolchain are available.
fn sysroot() -> Option<PathBuf> {
    let toolchain = discover_toolchain();
    toolchain.lean.as_ref()?;
    lean_sysroot(&toolchain, &lean_policy(60), &CancelToken::new()).ok()
}

fn report_for(module: &str) -> Option<SidecarReport> {
    let Some(exe) = sidecar_exe() else {
        eprintln!("skipped: sidecar not built (run `lake build` in lean/tpt-lean-sidecar)");
        return None;
    };
    let Some(root) = sysroot() else {
        eprintln!("skipped: lean not installed");
        return None;
    };
    Some(run_sidecar(&exe, &root, module, None, &policy(), &CancelToken::new()).expect("sidecar run"))
}

#[test]
fn describes_init_prelude() {
    let Some(r) = report_for("Init.Prelude") else {
        return;
    };
    assert_eq!(r.module, "Init.Prelude");
    let nat_add = r
        .declarations
        .iter()
        .find(|d| d.name == "Nat.add")
        .expect("Nat.add is declared in Init.Prelude");
    assert_eq!(nat_add.kind, "definition");
    assert!(nat_add.type_.contains("Nat"), "{nat_add:?}");
    assert!(!nat_add.type_.contains('\n'), "types are one line: {nat_add:?}");
}

#[test]
fn unknown_module_is_reported_as_not_imported() {
    let Some(exe) = sidecar_exe() else {
        eprintln!("skipped: sidecar not built");
        return;
    };
    let Some(root) = sysroot() else {
        eprintln!("skipped: lean not installed");
        return;
    };
    let err = run_sidecar(
        &exe,
        &root,
        "No.Such.Module",
        None,
        &policy(),
        &CancelToken::new(),
    )
    .expect_err("import must fail");
    assert_eq!(err.code(), ErrorCode::ProjectNotFound, "{err}");
}

#[test]
fn invalid_module_name_never_spawns() {
    // Path validation happens before any process starts, so this needs no sidecar build.
    let err = run_sidecar(
        Path::new("definitely-not-a-binary"),
        Path::new("unused"),
        "../../etc/passwd",
        None,
        &policy(),
        &CancelToken::new(),
    )
    .expect_err("invalid name must be refused");
    assert_eq!(err.code(), ErrorCode::InvalidArgument, "{err}");
}
