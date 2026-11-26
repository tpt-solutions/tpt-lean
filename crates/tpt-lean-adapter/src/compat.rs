// SPDX-License-Identifier: MIT OR Apache-2.0
//! Which Lean versions this release has been tested against, and how another version compares.
//!
//! Output parsing and the LSP protocol are only checked against the versions in
//! [`TESTED_LEAN_VERSIONS`]. A different version is not refused: callers report it, so that a
//! mismatch explains odd output instead of looking like a tool bug.

use tpt_lean_core::policy::{CancelToken, ProcessPolicy};

use crate::{execute, parse_version, Command, Toolchain};

/// Lean versions this release has been tested against. Keep this to versions that were actually
/// run. The previous stable release should be added once it has been installed and tested.
pub const TESTED_LEAN_VERSIONS: &[&str] = &["4.35.0", "4.36.0"];

/// How a Lean version compares with [`TESTED_LEAN_VERSIONS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compatibility {
    /// Exactly one of the tested versions.
    Tested,
    /// Newer than every tested version.
    Newer,
    /// Older than every tested version.
    Older,
    /// Between tested versions, but not one of them.
    Between,
    /// The text is not a version this module understands.
    Unknown,
}

/// `(major, minor, patch)` from text such as `4.36.0`, `v4.36.0`, `4.36`, or `4.36.0-rc1`.
/// A missing patch counts as zero. Anything after `-` (a pre-release tag) is ignored.
pub fn version_triple(text: &str) -> Option<(u64, u64, u64)> {
    let core = text.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = match parts.next() {
        Some(p) => p.parse().ok()?,
        None => 0,
    };
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// Classify a Lean version against [`TESTED_LEAN_VERSIONS`].
pub fn classify(version: &str) -> Compatibility {
    let Some(v) = version_triple(version) else {
        return Compatibility::Unknown;
    };
    let tested: Vec<(u64, u64, u64)> = TESTED_LEAN_VERSIONS
        .iter()
        .filter_map(|t| version_triple(t))
        .collect();
    if tested.contains(&v) {
        return Compatibility::Tested;
    }
    match (tested.iter().min(), tested.iter().max()) {
        (Some(&lo), _) if v < lo => Compatibility::Older,
        (_, Some(&hi)) if v > hi => Compatibility::Newer,
        (Some(_), Some(_)) => Compatibility::Between,
        _ => Compatibility::Unknown,
    }
}

impl Compatibility {
    /// A short name for output: `tested`, `newer`, `older`, `between` or `unknown`.
    pub fn as_str(self) -> &'static str {
        match self {
            Compatibility::Tested => "tested",
            Compatibility::Newer => "newer",
            Compatibility::Older => "older",
            Compatibility::Between => "between",
            Compatibility::Unknown => "unknown",
        }
    }
}

/// The installed Lean version (from `lean --version`) and how it compares with the tested set.
/// The version is `None` when it cannot be read, and the classification is then `Unknown`.
pub fn installed(
    toolchain: &Toolchain,
    policy: &ProcessPolicy,
    cancel: &CancelToken,
) -> (Option<String>, Compatibility) {
    let version = toolchain
        .lean
        .as_ref()
        .and_then(|_| execute(toolchain, &Command::LeanVersion, None, policy, cancel).ok())
        .and_then(|r| parse_version(&r.output.stdout));
    let classification = match &version {
        Some(v) => classify(v),
        None => Compatibility::Unknown,
    };
    (version, classification)
}

/// The Lean version a `lean-toolchain` file pins, as a plain version: `leanprover/lean4:v4.36.0`
/// gives `4.36.0`. Channel names such as `stable` pin no version and give `None`.
pub fn pinned_version(toolchain_file: &str) -> Option<String> {
    let tag = toolchain_file.trim().rsplit(':').next()?;
    let plain = tag.trim_start_matches('v');
    version_triple(plain).map(|_| plain.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_version_spellings() {
        assert_eq!(version_triple("4.36.0"), Some((4, 36, 0)));
        assert_eq!(version_triple("v4.36.0"), Some((4, 36, 0)));
        assert_eq!(version_triple("4.36"), Some((4, 36, 0)));
        assert_eq!(version_triple("4.36.0-rc1"), Some((4, 36, 0)));
        assert_eq!(version_triple("stable"), None);
        assert_eq!(version_triple("4.x.0"), None);
        assert_eq!(version_triple("1.2.3.4"), None);
    }

    #[test]
    fn classifies_against_tested_versions() {
        assert_eq!(classify("4.36.0"), Compatibility::Tested);
        assert_eq!(classify("4.37.0"), Compatibility::Newer);
        assert_eq!(classify("4.9.0"), Compatibility::Older);
        assert_eq!(classify("garbage"), Compatibility::Unknown);
    }

    #[test]
    fn between_is_only_possible_with_several_tested_versions() {
        // With the two tested versions 4.35.0 and 4.36.0, a version in between is `between`,
        // and one past the newest tested version is `newer`, never `between`.
        assert!(TESTED_LEAN_VERSIONS.len() >= 2);
        assert_eq!(classify("4.35.5"), Compatibility::Between);
        assert_ne!(classify("4.36.1"), Compatibility::Between);
    }

    #[test]
    fn pinned_version_reads_tagged_toolchains_only() {
        assert_eq!(
            pinned_version("leanprover/lean4:v4.36.0\n").as_deref(),
            Some("4.36.0")
        );
        assert_eq!(pinned_version("leanprover/lean4:stable"), None);
        assert_eq!(
            pinned_version("leanprover/lean4:v4.36.0-rc1").as_deref(),
            Some("4.36.0-rc1")
        );
    }
}
