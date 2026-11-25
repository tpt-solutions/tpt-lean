// SPDX-License-Identifier: MIT OR Apache-2.0
//! Permission classes for API methods.
//!
//! Every request is checked against the class its method needs before dispatch:
//! - `read`: no side effects on the project or the index.
//! - `write`: changes files (`format.file`) or the index in `.tpt-lean/` (`index.refresh`).
//!   Granted only when `security.allow_write` is true (default false).
//! - `build`: runs Lean (`check.file`, `goals.at`). Granted when `security.allow_build` is true
//!   (default true).
//! - `profile`: runs Lean on instrumented copies of files (`profile.run`). Granted only when
//!   both `security.allow_build` and `security.allow_profile` are true.
//! - `control`: lifecycle only (`server.shutdown`). Always granted; the server only listens on
//!   loopback unless `server.allow_remote` is set.

use tpt_lean_core::config::Config;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    Read,
    Write,
    Build,
    Profile,
    Control,
}

impl Permission {
    pub fn as_str(self) -> &'static str {
        match self {
            Permission::Read => "read",
            Permission::Write => "write",
            Permission::Build => "build",
            Permission::Profile => "profile",
            Permission::Control => "control",
        }
    }

    /// The configuration key that grants this class, when the class is configurable.
    pub fn config_key(self) -> Option<&'static str> {
        match self {
            Permission::Write => Some("security.allow_write"),
            Permission::Build => Some("security.allow_build"),
            Permission::Profile => Some("security.allow_build and security.allow_profile"),
            Permission::Read | Permission::Control => None,
        }
    }

    /// Whether the configuration grants this class.
    pub fn granted(self, config: &Config) -> bool {
        match self {
            Permission::Read | Permission::Control => true,
            Permission::Write => config.security.allow_write,
            Permission::Build => config.security.allow_build,
            Permission::Profile => config.security.allow_build && config.security.allow_profile,
        }
    }
}

/// Every method the server dispatches, with the permission class it requires.
pub const METHODS: &[(&str, Permission)] = &[
    ("project.info", Permission::Read),
    ("index.status", Permission::Read),
    ("search.query", Permission::Read),
    ("search.symbol", Permission::Read),
    ("format.text", Permission::Read),
    ("diagnostics.get", Permission::Read),
    ("format.file", Permission::Write),
    ("index.refresh", Permission::Write),
    ("check.file", Permission::Build),
    ("goals.at", Permission::Build),
    ("profile.run", Permission::Profile),
    ("profile.get", Permission::Read),
    ("build.run", Permission::Build),
    ("build.status", Permission::Read),
    ("build.cancel", Permission::Build),
    ("check.project", Permission::Build),
    ("diagnostics.explain", Permission::Build),
    ("imports.suggest", Permission::Read),
    ("imports.unused", Permission::Read),
    ("server.shutdown", Permission::Control),
];

/// The permission class a method requires, or `None` for an unknown method.
pub fn lookup(method: &str) -> Option<Permission> {
    METHODS.iter().find(|(name, _)| *name == method).map(|(_, p)| *p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_and_build_follow_config() {
        let mut cfg = Config::default();
        assert!(!Permission::Write.granted(&cfg));
        assert!(Permission::Build.granted(&cfg));
        cfg.security.allow_write = true;
        cfg.security.allow_build = false;
        assert!(Permission::Write.granted(&cfg));
        assert!(!Permission::Build.granted(&cfg));
        assert!(Permission::Read.granted(&cfg));
        assert!(Permission::Control.granted(&cfg));
    }

    #[test]
    fn profile_needs_both_switches() {
        let mut cfg = Config::default();
        assert!(Permission::Profile.granted(&cfg));
        cfg.security.allow_profile = false;
        assert!(!Permission::Profile.granted(&cfg));
        assert!(Permission::Build.granted(&cfg));
        cfg.security.allow_profile = true;
        cfg.security.allow_build = false;
        assert!(!Permission::Profile.granted(&cfg));
        assert_eq!(lookup("profile.run"), Some(Permission::Profile));
    }

    #[test]
    fn lookup_knows_every_method_once() {
        assert_eq!(lookup("format.file"), Some(Permission::Write));
        assert_eq!(lookup("check.file"), Some(Permission::Build));
        assert_eq!(lookup("nope"), None);
        let mut names: Vec<&str> = METHODS.iter().map(|(n, _)| *n).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), METHODS.len());
    }
}
