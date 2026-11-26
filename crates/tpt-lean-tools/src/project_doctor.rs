// SPDX-License-Identifier: MIT OR Apache-2.0
//! Project health checks: toolchain, project marker, configuration, index, cache, language
//! server. `tpt-lean doctor` and the server's `project.doctor` both run these, so the two report
//! the same things. Failures are reported as data; the caller decides the exit status.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tpt_lean_adapter::{self as adapter, Command as LeanCommand};
use tpt_lean_core::dirs::{global_cache_dir, ProjectDirs};
use tpt_lean_core::policy::CancelToken;
use tpt_lean_core::project;
use tpt_lean_index::Index;

use crate::doctor::{doctor_extensions, CheckStatus};

/// One health check. `status` is `ok`, `warn`, `fail`, or `skipped`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectCheck {
    pub name: &'static str,
    pub status: &'static str,
    pub detail: String,
}

/// Run every project check for `root`. `config` is the outcome of loading the configuration.
pub fn project_checks(root: &Path, config: std::result::Result<(), String>) -> Vec<ProjectCheck> {
    let mut checks: Vec<ProjectCheck> = Vec::new();
    let tc = adapter::discover_toolchain();
    let cancel = CancelToken::new();
    let policy = adapter::lean_policy(10);

    checks.push(match &tc.lean {
        Some(_) => match adapter::execute(&tc, &LeanCommand::LeanVersion, None, &policy, &cancel) {
            Ok(r) => ProjectCheck {
                name: "lean",
                status: "ok",
                detail: adapter::parse_version(&r.output.stdout)
                    .unwrap_or_else(|| r.output.stdout.trim().to_string()),
            },
            Err(e) => ProjectCheck {
                name: "lean",
                status: "fail",
                detail: e.to_string(),
            },
        },
        None => ProjectCheck {
            name: "lean",
            status: "fail",
            detail: "lean not found on PATH or in ~/.elan/bin".into(),
        },
    });
    checks.push(match &tc.lake {
        Some(_) => match adapter::execute(&tc, &LeanCommand::LakeVersion, None, &policy, &cancel) {
            Ok(r) => ProjectCheck {
                name: "lake",
                status: "ok",
                detail: adapter::parse_version(&r.output.stdout)
                    .unwrap_or_else(|| r.output.stdout.trim().to_string()),
            },
            Err(e) => ProjectCheck {
                name: "lake",
                status: "fail",
                detail: e.to_string(),
            },
        },
        None => ProjectCheck {
            name: "lake",
            status: "fail",
            detail: "lake not found on PATH or in ~/.elan/bin".into(),
        },
    });

    match project::discover(root) {
        Ok(p) => {
            checks.push(ProjectCheck {
                name: "project",
                status: "ok",
                detail: format!("found {} at {}", p.marker, p.root.display()),
            });
            match p.toolchain {
                Some(t) => {
                    checks.push(ProjectCheck {
                        name: "toolchain",
                        status: "ok",
                        detail: format!("lean-toolchain pins {t}"),
                    });
                    checks.push(lean_compat_check(&tc, &t));
                }
                None => checks.push(ProjectCheck {
                    name: "toolchain",
                    status: "warn",
                    detail: "no lean-toolchain file".into(),
                }),
            }
        }
        Err(_) => checks.push(ProjectCheck {
            name: "project",
            status: "warn",
            detail: format!("no project marker above {}", root.display()),
        }),
    }

    match &config {
        Ok(()) => checks.push(ProjectCheck {
            name: "config",
            status: "ok",
            detail: "configuration loads".into(),
        }),
        Err(e) => checks.push(ProjectCheck {
            name: "config",
            status: "fail",
            detail: e.clone(),
        }),
    }

    let dirs = ProjectDirs::new(root);
    match dirs.ensure() {
        Ok(()) => checks.push(ProjectCheck {
            name: "write",
            status: "ok",
            detail: format!("{} is writable", dirs.root.display()),
        }),
        Err(e) => checks.push(ProjectCheck {
            name: "write",
            status: "fail",
            detail: e.to_string(),
        }),
    }

    if dirs.index_db().exists() {
        match Index::open(&dirs.index_db()).and_then(|i| i.status()) {
            Ok(s) if s.integrity_ok => checks.push(ProjectCheck {
                name: "index",
                status: "ok",
                detail: format!("{} files, {} symbols", s.files, s.symbols),
            }),
            Ok(_) => checks.push(ProjectCheck {
                name: "index",
                status: "fail",
                detail: "integrity check failed; run `index clear` then `index scan`".into(),
            }),
            Err(e) => checks.push(ProjectCheck {
                name: "index",
                status: "fail",
                detail: e.to_string(),
            }),
        }
    } else {
        checks.push(ProjectCheck {
            name: "index",
            status: "warn",
            detail: "not built yet; run `tpt-lean index scan`".into(),
        });
    }

    let mathlib = root.join(".lake").join("packages").join("mathlib");
    checks.push(if mathlib.is_dir() {
        ProjectCheck {
            name: "mathlib",
            status: "ok",
            detail: "present in .lake/packages".into(),
        }
    } else {
        ProjectCheck {
            name: "mathlib",
            status: "warn",
            detail: "not present (only needed for Mathlib projects)".into(),
        }
    });

    match global_cache_dir() {
        Some(d) => checks.push(ProjectCheck {
            name: "cache_dir",
            status: "ok",
            detail: d.display().to_string(),
        }),
        None => checks.push(ProjectCheck {
            name: "cache_dir",
            status: "warn",
            detail: "no cache directory could be determined".into(),
        }),
    }

    checks.push(config_file_check(root));
    checks.push(sidecar_check(root));

    for c in doctor_extensions(root) {
        let status = match c.status {
            CheckStatus::Ok => "ok",
            CheckStatus::Warn => "warn",
            CheckStatus::Fail => "fail",
            CheckStatus::Skipped => "skipped",
        };
        let detail = match &c.suggestion {
            Some(s) => format!("{} (suggestion: {s})", c.detail),
            None => c.detail.clone(),
        };
        checks.push(ProjectCheck {
            name: c.name,
            status,
            detail,
        });
    }
    checks
}

/// The Lean sidecar source directory, relative to a tpt-lean checkout.
pub const SIDECAR_SOURCE: &str = "lean/tpt-lean-sidecar";

/// Where `lake build` puts the sidecar executable for the default build directory.
pub fn built_sidecar_exe(root: &Path) -> PathBuf {
    root.join(SIDECAR_SOURCE)
        .join(".lake")
        .join("build")
        .join("bin")
        .join(format!("tpt_sidecar{}", std::env::consts::EXE_SUFFIX))
}

/// Whether the project has a configuration file. Without one the built-in defaults are used,
/// which is a valid setup, so this is a warning that `doctor --fix` can resolve.
fn config_file_check(root: &Path) -> ProjectCheck {
    let dirs = ProjectDirs::new(root);
    if root.join("tpt-lean.toml").is_file() || dirs.config().is_file() {
        ProjectCheck {
            name: "config-file",
            status: "ok",
            detail: "a configuration file is present".into(),
        }
    } else {
        ProjectCheck {
            name: "config-file",
            status: "warn",
            detail: "no configuration file; built-in defaults are in use".into(),
        }
    }
}

/// The Lean sidecar used by `index sidecar` and `goals --sidecar`. It comes from
/// `TPT_LEAN_SIDECAR`, or is built from `lean/tpt-lean-sidecar` when this is a checkout of the
/// tool itself.
fn sidecar_check(root: &Path) -> ProjectCheck {
    if let Some(p) = std::env::var_os("TPT_LEAN_SIDECAR").map(PathBuf::from) {
        return if p.is_file() {
            ProjectCheck {
                name: "sidecar",
                status: "ok",
                detail: format!("TPT_LEAN_SIDECAR is {}", p.display()),
            }
        } else {
            ProjectCheck {
                name: "sidecar",
                status: "fail",
                detail: format!("TPT_LEAN_SIDECAR points at a missing file: {}", p.display()),
            }
        };
    }
    if !root.join(SIDECAR_SOURCE).join("lakefile.lean").is_file() {
        return ProjectCheck {
            name: "sidecar",
            status: "skipped",
            detail: "no sidecar source in this project; set TPT_LEAN_SIDECAR to a built tpt_sidecar"
                .into(),
        };
    }
    let exe = built_sidecar_exe(root);
    if exe.is_file() {
        ProjectCheck {
            name: "sidecar",
            status: "ok",
            detail: format!("built at {}", exe.display()),
        }
    } else {
        ProjectCheck {
            name: "sidecar",
            status: "warn",
            detail: format!("{SIDECAR_SOURCE} is not built; doctor --fix builds it with lake build"),
        }
    }
}

/// Compare the Lean found on PATH with the project's pin and with the versions this release
/// is tested against. A mismatch is a warning, not a failure: the tools still run.
fn lean_compat_check(tc: &adapter::Toolchain, pinned_file: &str) -> ProjectCheck {
    let policy = adapter::lean_policy(10);
    let installed = tc
        .lean
        .as_ref()
        .and_then(|_| {
            adapter::execute(tc, &LeanCommand::LeanVersion, None, &policy, &CancelToken::new()).ok()
        })
        .and_then(|r| adapter::parse_version(&r.output.stdout));
    let Some(installed) = installed else {
        return ProjectCheck {
            name: "lean-version",
            status: "warn",
            detail: "could not read the installed Lean version".into(),
        };
    };
    let mut notes: Vec<String> = Vec::new();
    if let Some(pinned) = adapter::compat::pinned_version(pinned_file) {
        if pinned != installed {
            notes.push(format!(
                "lean on PATH is {installed} but lean-toolchain pins {pinned}"
            ));
        }
    }
    let tested = adapter::compat::TESTED_LEAN_VERSIONS.join(", ");
    let status = match adapter::compat::classify(&installed) {
        adapter::compat::Compatibility::Tested => "ok",
        adapter::compat::Compatibility::Newer => {
            notes.push(format!(
                "{installed} is newer than the tested versions ({tested}); output may differ"
            ));
            "warn"
        }
        adapter::compat::Compatibility::Older => {
            notes.push(format!(
                "{installed} is older than the tested versions ({tested}); output may differ"
            ));
            "warn"
        }
        adapter::compat::Compatibility::Between | adapter::compat::Compatibility::Unknown => {
            notes.push(format!("{installed} is not a tested version ({tested})"));
            "warn"
        }
    };
    let detail = if notes.is_empty() {
        format!("{installed} is a tested version and matches the pin")
    } else {
        notes.join("; ")
    };
    ProjectCheck {
        name: "lean-version",
        status: if notes.is_empty() { status } else { "warn" },
        detail,
    }
}
