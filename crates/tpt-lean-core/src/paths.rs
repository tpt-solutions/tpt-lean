// SPDX-License-Identifier: MIT OR Apache-2.0
//! Path normalization and workspace-root safety checks.
//!
//! Every path that arrives from an API, MCP, or FFI request must pass through
//! [`WorkspaceRoot::resolve`] before it is used.

use crate::error::{Error, ErrorCode, Result};
use std::path::{Component, Path, PathBuf};

/// Strip the Windows verbatim prefix (`\\?\` or `\\?\UNC\`) and normalise separators.
pub fn strip_verbatim(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    path.to_path_buf()
}

/// Lexically normalise a path: resolve `.` and `..` without touching the filesystem.
/// `..` that would escape the root of a relative path is preserved so callers can reject it.
pub fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out: Vec<Component<'_>> = Vec::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => match out.last() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir) | Some(Component::Prefix(_)) => {}
                _ => out.push(comp),
            },
            other => out.push(other),
        }
    }
    let mut buf = PathBuf::new();
    for c in out {
        buf.push(c.as_os_str());
    }
    if buf.as_os_str().is_empty() {
        buf.push(".");
    }
    buf
}

/// Case-insensitive comparison on Windows, case-sensitive elsewhere (macOS default is
/// case-insensitive too, but canonical paths are compared exactly after `canonicalize`).
fn starts_with(path: &Path, root: &Path) -> bool {
    #[cfg(windows)]
    {
        let p = path.to_string_lossy().to_lowercase();
        let r = root.to_string_lossy().to_lowercase();
        let r = r.trim_end_matches(['\\', '/']).to_string();
        p == r || p.starts_with(&format!("{r}\\")) || p.starts_with(&format!("{r}/"))
    }
    #[cfg(not(windows))]
    {
        path.starts_with(root)
    }
}

/// A canonical workspace root. All resolved paths must live under it.
#[derive(Debug, Clone)]
pub struct WorkspaceRoot {
    root: PathBuf,
}

impl WorkspaceRoot {
    /// Create a workspace root from an existing directory.
    pub fn new(root: &Path) -> Result<Self> {
        let canon = std::fs::canonicalize(root).map_err(|e| Error::from(e).with_path(root))?;
        Ok(Self {
            root: strip_verbatim(&canon),
        })
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    /// Resolve a user-supplied path (relative to the root, or absolute) and verify that the
    /// final, symlink-resolved location is inside the workspace. Files that do not exist yet
    /// are checked by resolving their nearest existing ancestor.
    pub fn resolve(&self, input: &str) -> Result<PathBuf> {
        if input.contains('\0') {
            return Err(Error::invalid("path contains NUL byte"));
        }
        let raw = Path::new(input);
        let joined = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            self.root.join(raw)
        };
        let normalized = lexical_normalize(&joined);
        if !normalized.is_absolute() {
            return Err(Error::new(ErrorCode::PathOutsideWorkspace, "path is not absolute").with_path(input));
        }

        // Canonicalize the deepest existing ancestor so symlinks cannot escape the root.
        let mut existing = normalized.as_path();
        let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
        while !existing.exists() {
            match (existing.parent(), existing.file_name()) {
                (Some(parent), Some(name)) => {
                    tail.push(name);
                    existing = parent;
                }
                _ => break,
            }
        }
        let mut resolved = match std::fs::canonicalize(existing) {
            Ok(p) => strip_verbatim(&p),
            Err(_) => existing.to_path_buf(),
        };
        for name in tail.iter().rev() {
            resolved.push(name);
        }

        if !starts_with(&resolved, &self.root) {
            return Err(
                Error::new(ErrorCode::PathOutsideWorkspace, "path escapes the workspace root")
                    .with_path(input),
            );
        }
        if is_device_name(&resolved) {
            return Err(
                Error::new(ErrorCode::PathOutsideWorkspace, "device files are not allowed").with_path(input),
            );
        }
        Ok(resolved)
    }
}

/// Windows reserved device names (CON, NUL, COM1, ...). Harmless no-op on other platforms.
fn is_device_name(path: &Path) -> bool {
    let Some(stem) = path.file_stem().map(|s| s.to_string_lossy().to_uppercase()) else {
        return false;
    };
    const RESERVED: &[&str] = &["CON", "PRN", "AUX", "NUL"];
    if RESERVED.contains(&stem.as_str()) {
        return true;
    }
    if let Some(num) = stem.strip_prefix("COM").or_else(|| stem.strip_prefix("LPT")) {
        return num.len() == 1 && num.chars().all(|c| c.is_ascii_digit() && c != '0');
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_resolves_dots() {
        assert_eq!(
            lexical_normalize(Path::new("a/b/../c/./d")),
            PathBuf::from("a").join("c").join("d")
        );
    }

    #[test]
    fn normalize_keeps_leading_parent() {
        assert_eq!(
            lexical_normalize(Path::new("../x")),
            PathBuf::from("..").join("x")
        );
    }

    #[test]
    fn strips_verbatim_prefix() {
        assert_eq!(strip_verbatim(Path::new(r"\\?\C:\x")), PathBuf::from(r"C:\x"));
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\UNC\srv\share")),
            PathBuf::from(r"\\srv\share")
        );
    }

    #[test]
    fn rejects_escape_and_accepts_inside() {
        let dir = std::env::temp_dir().join(format!("tpt-ws-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let ws = WorkspaceRoot::new(&dir).unwrap();
        assert!(ws.resolve("src/Main.lean").is_ok());
        assert!(ws.resolve("src/New/File.lean").is_ok());
        let err = ws.resolve("../../etc/passwd").unwrap_err();
        assert_eq!(err.code(), ErrorCode::PathOutsideWorkspace);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_device_names() {
        assert!(is_device_name(Path::new("x/NUL")));
        assert!(is_device_name(Path::new("COM3.txt")));
        assert!(!is_device_name(Path::new("COM0")));
        assert!(!is_device_name(Path::new("Main.lean")));
    }

    #[test]
    fn rejects_nul_bytes() {
        let dir = std::env::temp_dir();
        let ws = WorkspaceRoot::new(&dir).unwrap();
        assert_eq!(ws.resolve("a\0b").unwrap_err().code(), ErrorCode::InvalidArgument);
    }
}
