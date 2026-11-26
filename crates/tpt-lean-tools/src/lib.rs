// SPDX-License-Identifier: MIT OR Apache-2.0
//! Utilities built on the tpt-lean components: import analysis, git commit bisect, the CI
//! runner with SARIF output, and doctor extensions.
//!
//! Everything here is a library entry point. The CLI crate decides how to wire it up.
//! Only [`fix::fix_file`] with `apply` set writes a project file; everything else only reads. See
//! `docs/tools.md` for the limits of each part.

pub mod bisect;
pub mod ci;
pub mod doctor;
pub mod fix;
pub mod imports;
pub mod project_doctor;
pub mod sorry;
pub mod upgrade;

use serde::Serialize;

/// A single analysis result with a stable machine-readable `code`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finding {
    pub code: &'static str,
    /// `warning`, `info` or `error`.
    pub severity: &'static str,
    /// Project-relative path with forward slashes.
    pub file: String,
    /// 1-based line, or 0 when the finding is not tied to a line.
    pub line: usize,
    pub message: String,
}

/// Relative path (forward slashes) of `path` under `root`, or the path itself when outside.
pub(crate) fn relative_display(root: &std::path::Path, path: &std::path::Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}
