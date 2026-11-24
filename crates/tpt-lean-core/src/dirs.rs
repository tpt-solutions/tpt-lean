// SPDX-License-Identifier: MIT OR Apache-2.0
//! Project-local data layout (`.tpt-lean/`) and the per-OS global cache directory.

use std::path::{Path, PathBuf};

/// Paths inside a project's `.tpt-lean/` directory (specification section 14.1).
#[derive(Debug, Clone)]
pub struct ProjectDirs {
    pub root: PathBuf,
}

impl ProjectDirs {
    pub fn new(project_root: &Path) -> Self {
        Self {
            root: project_root.join(".tpt-lean"),
        }
    }

    pub fn config(&self) -> PathBuf {
        self.root.join("config.toml")
    }
    pub fn index_db(&self) -> PathBuf {
        self.root.join("index.sqlite")
    }
    pub fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }
    pub fn profiles(&self) -> PathBuf {
        self.root.join("profiles")
    }
    pub fn runs(&self) -> PathBuf {
        self.root.join("runs")
    }
    pub fn tmp(&self) -> PathBuf {
        self.root.join("tmp")
    }

    /// Create the directory tree (idempotent).
    pub fn ensure(&self) -> std::io::Result<()> {
        for d in [
            self.root.clone(),
            self.logs(),
            self.profiles(),
            self.runs(),
            self.tmp(),
        ] {
            std::fs::create_dir_all(d)?;
        }
        Ok(())
    }
}

/// Global cache directory for the current OS (specification section 14.2).
/// Linux: `$XDG_CACHE_HOME/tpt-lean` (or `~/.cache/tpt-lean`); macOS: `~/Library/Caches/tpt-lean`;
/// Windows: `%LOCALAPPDATA%\tpt-lean\cache`.
pub fn global_cache_dir() -> Option<PathBuf> {
    global_cache_dir_from(|k| std::env::var_os(k).map(PathBuf::from))
}

/// Testable core of [`global_cache_dir`]; `get` looks up an environment variable.
pub fn global_cache_dir_from(get: impl Fn(&str) -> Option<PathBuf>) -> Option<PathBuf> {
    if cfg!(windows) {
        get("LOCALAPPDATA").map(|p| p.join("tpt-lean").join("cache"))
    } else if cfg!(target_os = "macos") {
        get("HOME").map(|h| h.join("Library").join("Caches").join("tpt-lean"))
    } else {
        get("XDG_CACHE_HOME")
            .map(|p| p.join("tpt-lean"))
            .or_else(|| get("HOME").map(|h| h.join(".cache").join("tpt-lean")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_layout() {
        let d = ProjectDirs::new(Path::new("proj"));
        assert!(d.index_db().ends_with("index.sqlite"));
        assert!(d.index_db().starts_with(d.root.clone()));
    }

    #[test]
    fn cache_dir_uses_env() {
        let got = global_cache_dir_from(|k| match k {
            "LOCALAPPDATA" => Some(PathBuf::from("L")),
            "XDG_CACHE_HOME" => Some(PathBuf::from("X")),
            "HOME" => Some(PathBuf::from("H")),
            _ => None,
        });
        assert!(got.is_some());
    }
}
