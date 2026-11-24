// SPDX-License-Identifier: MIT OR Apache-2.0
//! Process execution policy: timeouts, output limits, cancellation, and env sanitization.
//!
//! This module only describes policy. The adapter crate enforces it when spawning Lean/Lake.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Limits applied to every external process.
#[derive(Debug, Clone)]
pub struct ProcessPolicy {
    pub timeout: Duration,
    /// Upper bound on captured stdout+stderr bytes.
    pub output_limit_bytes: usize,
    /// Memory limit in bytes where the platform supports it (`None` = unlimited or unsupported).
    pub memory_limit_bytes: Option<u64>,
    /// Sample the process tree's resident memory and report its peak, even without a limit.
    /// Sampling is periodic, so a short spike between samples can be missed.
    pub track_memory: bool,
    /// Environment variables allowed through. Everything else is removed.
    pub env_allowlist: Vec<String>,
    /// Variables set explicitly by the caller, applied after the allowlist. Use only for
    /// values the caller computed itself (for example `LEAN_SYSROOT`), never copied from input.
    pub extra_env: Vec<(String, String)>,
}

impl Default for ProcessPolicy {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(120),
            output_limit_bytes: 8 * 1024 * 1024,
            memory_limit_bytes: None,
            track_memory: false,
            env_allowlist: default_env_allowlist(),
            extra_env: Vec::new(),
        }
    }
}

/// Environment variables that Lean and Lake need to locate toolchains and their own files.
pub fn default_env_allowlist() -> Vec<String> {
    [
        "PATH",
        "HOME",
        "USERPROFILE",
        "SYSTEMROOT",
        "TEMP",
        "TMP",
        "TMPDIR",
        "LANG",
        "LC_ALL",
        "ELAN_HOME",
        "ELAN_TOOLCHAIN",
        "LEAN_PATH",
        "LAKE_HOME",
        "XDG_CACHE_HOME",
        "LOCALAPPDATA",
        "APPDATA",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect()
}

/// Cooperative cancellation flag shared between a caller and a running process.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Keep only the `(key, value)` pairs whose key is on the allowlist (case-insensitive on Windows).
pub fn sanitize_env<I>(vars: I, allowlist: &[String]) -> Vec<(String, String)>
where
    I: IntoIterator<Item = (String, String)>,
{
    vars.into_iter()
        .filter(|(k, _)| {
            allowlist.iter().any(|a| {
                if cfg!(windows) {
                    a.eq_ignore_ascii_case(k)
                } else {
                    a == k
                }
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_drops_unlisted_vars() {
        let allow = vec!["PATH".to_string()];
        let out = sanitize_env(
            vec![
                ("PATH".into(), "/bin".into()),
                ("SECRET_TOKEN".into(), "x".into()),
            ],
            &allow,
        );
        assert_eq!(out, vec![("PATH".to_string(), "/bin".to_string())]);
    }

    #[test]
    fn cancel_token_is_shared() {
        let t = CancelToken::new();
        let t2 = t.clone();
        assert!(!t2.is_cancelled());
        t.cancel();
        assert!(t2.is_cancelled());
    }
}
