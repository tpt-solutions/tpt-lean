// SPDX-License-Identifier: MIT OR Apache-2.0
//! Project discovery and Lean toolchain detection.

use crate::error::{Error, ErrorCode, Result};
use std::path::{Path, PathBuf};

/// Files that mark the root of a Lean project, in priority order.
pub const MARKER_FILES: &[&str] = &["tpt-lean.toml", "lakefile.lean", "lakefile.toml"];

/// A discovered project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub root: PathBuf,
    /// The marker file that identified the root.
    pub marker: String,
    /// Contents of `lean-toolchain`, trimmed, if present.
    pub toolchain: Option<String>,
}

/// Walk upwards from `start` until a directory containing a marker file is found.
pub fn discover(start: &Path) -> Result<Project> {
    let start = if start.is_file() {
        start.parent().unwrap_or(start)
    } else {
        start
    };
    let start = crate::paths::strip_verbatim(
        &std::fs::canonicalize(start).map_err(|e| Error::from(e).with_path(start))?,
    );
    let mut current: Option<&Path> = Some(&start);
    while let Some(dir) = current {
        for marker in MARKER_FILES {
            if dir.join(marker).is_file() {
                return Ok(Project {
                    root: dir.to_path_buf(),
                    marker: (*marker).to_string(),
                    toolchain: read_toolchain_file(dir),
                });
            }
        }
        current = dir.parent();
    }
    Err(Error::new(
        ErrorCode::ProjectNotFound,
        "no tpt-lean.toml, lakefile.lean or lakefile.toml found",
    )
    .with_path(start))
}

/// Read and trim `lean-toolchain` in `dir`, returning `None` if absent or empty.
pub fn read_toolchain_file(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("lean-toolchain")).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_marker_in_parent() {
        let base = std::env::temp_dir().join(format!("tpt-proj-{}", std::process::id()));
        let nested = base.join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(base.join("lakefile.toml"), "").unwrap();
        std::fs::write(base.join("lean-toolchain"), "leanprover/lean4:v4.9.0\n").unwrap();
        let p = discover(&nested).unwrap();
        assert_eq!(p.marker, "lakefile.toml");
        assert_eq!(p.toolchain.as_deref(), Some("leanprover/lean4:v4.9.0"));
        let _ = std::fs::remove_dir_all(&base);
    }
}
