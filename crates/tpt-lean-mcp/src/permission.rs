// SPDX-License-Identifier: MIT OR Apache-2.0
//! Permission classes for MCP tools (specification section 12.2).
//!
//! Every tool declares one class. The server checks it against `[security]` in the project
//! config before the tool runs, so a denied call never touches the filesystem or a process.

use tpt_lean_core::config::SecurityConfig;

/// The permission class a tool needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    /// Reads the index, source text, or in-memory data. Always allowed.
    Read,
    /// Writes files in the workspace (`security.allow_write`).
    Write,
    /// Runs Lean or Lake (`security.allow_build`).
    Build,
    /// Runs controlled Lean checks with timing. There is no separate profile key yet, so it
    /// is gated by `security.allow_build`.
    Profile,
}

impl Permission {
    pub fn as_str(self) -> &'static str {
        match self {
            Permission::Read => "read",
            Permission::Write => "write",
            Permission::Build => "build",
            Permission::Profile => "profile",
        }
    }
}

/// `Ok` when the config grants `class`; otherwise a message that names the missing permission.
pub fn check(security: &SecurityConfig, class: Permission) -> Result<(), String> {
    let (granted, key) = match class {
        Permission::Read => (true, ""),
        Permission::Write => (security.allow_write, "security.allow_write"),
        Permission::Build => (security.allow_build, "security.allow_build"),
        Permission::Profile => (security.allow_build, "security.allow_build"),
    };
    if granted {
        Ok(())
    } else if class == Permission::Profile {
        Err(format!(
            "permission denied: this tool needs the profile permission, which is granted by {key} = true in .tpt-lean/config.toml (there is no separate profile setting yet)"
        ))
    } else {
        Err(format!(
            "permission denied: this tool needs the {} permission; set {key} = true in .tpt-lean/config.toml",
            class.as_str()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_is_always_granted() {
        let sec = SecurityConfig {
            allow_write: false,
            allow_build: false,
            ..Default::default()
        };
        assert!(check(&sec, Permission::Read).is_ok());
    }

    #[test]
    fn write_needs_allow_write() {
        let mut sec = SecurityConfig::default();
        let err = check(&sec, Permission::Write).unwrap_err();
        assert!(err.contains("write permission"));
        assert!(err.contains("security.allow_write"));
        sec.allow_write = true;
        assert!(check(&sec, Permission::Write).is_ok());
    }

    #[test]
    fn build_and_profile_need_allow_build() {
        let sec = SecurityConfig {
            allow_build: false,
            ..Default::default()
        };
        assert!(check(&sec, Permission::Build)
            .unwrap_err()
            .contains("build permission"));
        let err = check(&sec, Permission::Profile).unwrap_err();
        assert!(err.contains("security.allow_build"));
    }
}
