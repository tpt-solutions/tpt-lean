// SPDX-License-Identifier: MIT OR Apache-2.0
//! Content hashing and cache keys. Uses SHA-256 (MIT OR Apache-2.0 via `sha2`).

use crate::error::{Error, Result};
use sha2::{Digest, Sha256};
use std::path::Path;

/// Lowercase hex SHA-256 of a byte slice.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Lowercase hex SHA-256 of a file's contents.
pub fn file_sha256(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).map_err(|e| Error::from(e).with_path(path))?;
    Ok(sha256_hex(&bytes))
}

/// Inputs that determine whether a cached artifact is still valid.
/// Fields follow the cache-key design in the specification (section 7.6).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CacheKey {
    pub lean_version: String,
    pub package_versions: String,
    pub lakefile_hash: String,
    pub dependency_lock_hash: String,
    pub source_hash: String,
    pub import_hashes: Vec<String>,
    pub lean_flags: Vec<String>,
    pub env: Vec<(String, String)>,
    pub target_triple: String,
}

impl CacheKey {
    /// Canonical, order-stable serialization. Lists are sorted so ordering never changes the key.
    pub fn canonical(&self) -> String {
        let mut imports = self.import_hashes.clone();
        imports.sort();
        let mut flags = self.lean_flags.clone();
        flags.sort();
        let mut env = self.env.clone();
        env.sort();
        let env_str: Vec<String> = env.iter().map(|(k, v)| format!("{k}={v}")).collect();
        [
            format!("lean={}", self.lean_version),
            format!("packages={}", self.package_versions),
            format!("lakefile={}", self.lakefile_hash),
            format!("lock={}", self.dependency_lock_hash),
            format!("source={}", self.source_hash),
            format!("imports={}", imports.join(",")),
            format!("flags={}", flags.join(" ")),
            format!("env={}", env_str.join(";")),
            format!("target={}", self.target_triple),
        ]
        .join("\n")
    }

    /// The cache key as a hex digest.
    pub fn digest(&self) -> String {
        sha256_hex(self.canonical().as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn cache_key_is_order_insensitive_for_lists() {
        let a = CacheKey {
            import_hashes: vec!["x".into(), "y".into()],
            ..Default::default()
        };
        let b = CacheKey {
            import_hashes: vec!["y".into(), "x".into()],
            ..Default::default()
        };
        assert_eq!(a.digest(), b.digest());
    }

    #[test]
    fn cache_key_changes_with_toolchain() {
        let a = CacheKey {
            lean_version: "4.9.0".into(),
            ..Default::default()
        };
        let b = CacheKey {
            lean_version: "4.10.0".into(),
            ..Default::default()
        };
        assert_ne!(a.digest(), b.digest());
    }
}
