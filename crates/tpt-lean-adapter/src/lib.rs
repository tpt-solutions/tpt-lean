// SPDX-License-Identifier: MIT OR Apache-2.0
//! Controlled interface to official Lean tooling (`lean`, `lake`).
//!
//! Level 1 (CLI integration) is implemented here: toolchain discovery, version queries,
//! controlled `lake build` and `lean` invocations built from typed options, and parsing of
//! diagnostics. Commands are never assembled from strings; there is no shell.
//!
//! Level 2 (LSP client, with restart and cancellation) is in `lsp` and `session`. Level 3 (sidecar) is not implemented yet; see `todo.md`.

pub mod diagnostics;
pub mod last_diagnostics;
pub mod lemmas;
pub mod lsp;
pub mod runner;
pub mod session;

use std::path::{Path, PathBuf};
use std::time::Duration;
use tpt_lean_core::policy::{CancelToken, ProcessPolicy};
use tpt_lean_core::{Error, ErrorCode, Result};

pub use diagnostics::{parse_lean_output, Diagnostic, Severity};
pub use last_diagnostics::{
    last_diagnostics_path, load_last_diagnostics, save_last_diagnostics, LastDiagnostics,
};
pub use lemmas::suggest_lemmas;
pub use runner::{run, ProcessOutput};
pub use session::LspSession;

/// Locations of the Lean toolchain executables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toolchain {
    pub lean: Option<PathBuf>,
    pub lake: Option<PathBuf>,
}

/// Find `lean` and `lake` on `PATH`, then in `ELAN_HOME/bin` (default `~/.elan/bin`).
pub fn discover_toolchain() -> Toolchain {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if let Some(elan) = std::env::var_os("ELAN_HOME") {
        dirs.push(PathBuf::from(elan).join("bin"));
    } else if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        dirs.push(PathBuf::from(home).join(".elan").join("bin"));
    }
    Toolchain {
        lean: find_exe(&dirs, "lean"),
        lake: find_exe(&dirs, "lake"),
    }
}

fn find_exe(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    let exe_names: Vec<String> = if cfg!(windows) {
        vec![format!("{name}.exe"), name.to_string()]
    } else {
        vec![name.to_string()]
    };
    dirs.iter()
        .flat_map(|d| exe_names.iter().map(move |n| d.join(n)))
        .find(|p| p.is_file())
}

/// A validated Lake target name: letters, digits, `_`, `.` and `-`, no path separators.
pub fn validate_target(target: &str) -> Result<&str> {
    let ok = !target.is_empty()
        && target.len() <= 200
        && target
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '-'))
        && !target.starts_with('.');
    if ok {
        Ok(target)
    } else {
        Err(Error::invalid(format!("invalid Lake target: {target:?}")))
    }
}

/// Typed Lean/Lake invocations. Each variant maps to a fixed argument vector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    LeanVersion,
    LakeVersion,
    /// `lake env printenv` (prints the environment Lake would use).
    LakeEnvPrintenv,
    /// `lake build [target]`
    LakeBuild {
        target: Option<String>,
    },
    /// `lake clean` (removes build outputs of the Lake project).
    LakeClean,
    /// `lean <file>` with no extra flags. The file is passed as an absolute path.
    LeanCheck {
        file: PathBuf,
    },
}

impl Command {
    /// Which executable this command runs, and its arguments.
    pub fn to_invocation(&self, toolchain: &Toolchain) -> Result<(PathBuf, Vec<String>)> {
        let lean = || {
            toolchain
                .lean
                .clone()
                .ok_or_else(|| Error::new(ErrorCode::LeanNotFound, "lean not found on PATH"))
        };
        let lake = || {
            toolchain
                .lake
                .clone()
                .ok_or_else(|| Error::new(ErrorCode::LeanNotFound, "lake not found on PATH"))
        };
        Ok(match self {
            Command::LeanVersion => (lean()?, vec!["--version".into()]),
            Command::LakeVersion => (lake()?, vec!["--version".into()]),
            Command::LakeEnvPrintenv => (lake()?, vec!["env".into(), "printenv".into()]),
            Command::LakeBuild { target } => {
                let mut args = vec!["build".to_string()];
                if let Some(t) = target {
                    args.push(validate_target(t)?.to_string());
                }
                (lake()?, args)
            }
            Command::LeanCheck { file } => (lean()?, vec![file.display().to_string()]),
            Command::LakeClean => (lake()?, vec!["clean".into()]),
        })
    }
}

/// Result of running a typed command.
#[derive(Debug, Clone)]
pub struct CommandResult {
    pub output: ProcessOutput,
    pub diagnostics: Vec<Diagnostic>,
}

/// Run a typed command under a process policy. `cwd` is the Lake project root.
pub fn execute(
    toolchain: &Toolchain,
    command: &Command,
    cwd: Option<&Path>,
    policy: &ProcessPolicy,
    cancel: &CancelToken,
) -> Result<CommandResult> {
    let (program, args) = command.to_invocation(toolchain)?;
    let output = run(&program, &args, cwd, policy, cancel)?;
    let diagnostics = parse_lean_output(&format!("{}\n{}", output.stdout, output.stderr));
    Ok(CommandResult { output, diagnostics })
}

/// Parse the first line of `lean --version` / `lake --version` into a version string,
/// for example `Lean (version 4.9.0, ...)` → `4.9.0`.
pub fn parse_version(text: &str) -> Option<String> {
    let line = text.lines().find(|l| !l.trim().is_empty())?;
    if let Some(rest) = line.split("version ").nth(1) {
        let v: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-')
            .collect();
        if !v.is_empty() {
            return Some(v);
        }
    }
    None
}

/// Default policy for Lean commands, using the configured timeout in seconds.
pub fn lean_policy(timeout_seconds: u64) -> ProcessPolicy {
    ProcessPolicy {
        timeout: Duration::from_secs(timeout_seconds),
        ..ProcessPolicy::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_validation() {
        assert!(validate_target("Main").is_ok());
        assert!(validate_target("My.Lib-2").is_ok());
        assert!(validate_target("../etc").is_err());
        assert!(validate_target("a b").is_err());
        assert!(validate_target("").is_err());
    }

    #[test]
    fn invocation_is_typed_and_shell_free() {
        let tc = Toolchain {
            lean: Some(PathBuf::from("/x/lean")),
            lake: Some(PathBuf::from("/x/lake")),
        };
        let (p, args) = Command::LakeBuild {
            target: Some("Main".into()),
        }
        .to_invocation(&tc)
        .unwrap();
        assert_eq!(p, PathBuf::from("/x/lake"));
        assert_eq!(args, vec!["build".to_string(), "Main".to_string()]);
        assert!(Command::LakeBuild {
            target: Some("x; rm -rf /".into())
        }
        .to_invocation(&tc)
        .is_err());
    }

    #[test]
    fn missing_toolchain_is_reported() {
        let tc = Toolchain {
            lean: None,
            lake: None,
        };
        let err = Command::LeanVersion.to_invocation(&tc).unwrap_err();
        assert_eq!(err.code(), ErrorCode::LeanNotFound);
    }

    #[test]
    fn version_parsing() {
        assert_eq!(
            parse_version("Lean (version 4.9.0, commit abc, Release)\n").as_deref(),
            Some("4.9.0")
        );
        assert_eq!(parse_version("nonsense"), None);
    }
}
