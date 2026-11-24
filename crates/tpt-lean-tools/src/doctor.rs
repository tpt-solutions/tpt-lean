// SPDX-License-Identifier: MIT OR Apache-2.0
//! Doctor extensions: server socket health, language server health, and global cache age.
//!
//! These checks are read-only with respect to project files. The LSP check starts the language
//! server (`lake serve` or `lean --server`) and shuts it down again; the server itself may write
//! its own caches.

use serde::Serialize;
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::path::Path;
use std::time::{Duration, SystemTime};
use tpt_lean_adapter::lsp::{server_command, LspClient};
use tpt_lean_core::dirs::global_cache_dir;

/// Cache entries older than this many days produce a warning.
pub const CACHE_WARN_DAYS: u64 = 30;
const SOCKET_TIMEOUT: Duration = Duration::from_millis(500);
const LSP_START_TIMEOUT: Duration = Duration::from_secs(20);
const LSP_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const SERVER_JSON_LIMIT: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Ok,
    Warn,
    Fail,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    pub name: &'static str,
    pub status: CheckStatus,
    pub detail: String,
    pub suggestion: Option<String>,
}

fn check(
    name: &'static str,
    status: CheckStatus,
    detail: impl Into<String>,
    suggestion: Option<&str>,
) -> Check {
    Check {
        name,
        status,
        detail: detail.into(),
        suggestion: suggestion.map(String::from),
    }
}

/// All extension checks for `project_root`, in display order.
pub fn doctor_extensions(project_root: &Path) -> Vec<Check> {
    vec![
        server_socket_check(project_root),
        lsp_check(project_root),
        cache_age_check(global_cache_dir().as_deref(), SystemTime::now(), CACHE_WARN_DAYS),
    ]
}

/// Probe the server named in `.tpt-lean/server.json` on a loopback address.
///
/// The file is a JSON object with `port` (required) and `host` (optional, default `127.0.0.1`).
/// Non-loopback hosts are never probed.
pub fn server_socket_check(project_root: &Path) -> Check {
    const NAME: &str = "server socket";
    let path = project_root.join(".tpt-lean").join("server.json");
    if !path.is_file() {
        return check(
            NAME,
            CheckStatus::Skipped,
            "no .tpt-lean/server.json; server not configured",
            None,
        );
    }
    let text = match std::fs::metadata(&path) {
        Ok(m) if m.len() > SERVER_JSON_LIMIT => {
            return check(NAME, CheckStatus::Fail, "server.json is larger than 64 KiB", None);
        }
        Ok(_) => std::fs::read_to_string(&path),
        Err(e) => Err(e),
    };
    let Ok(text) = text else {
        return check(NAME, CheckStatus::Fail, "cannot read .tpt-lean/server.json", None);
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return check(NAME, CheckStatus::Fail, "server.json is not valid JSON", None);
    };
    let Some(port) = value
        .get("port")
        .and_then(|p| p.as_u64())
        .and_then(|p| u16::try_from(p).ok())
    else {
        return check(NAME, CheckStatus::Fail, "server.json has no valid `port`", None);
    };
    let host = value.get("host").and_then(|h| h.as_str()).unwrap_or("127.0.0.1");
    let ip: Option<IpAddr> = match host {
        "localhost" => Some(IpAddr::from([127, 0, 0, 1])),
        other => other.parse::<IpAddr>().ok(),
    };
    let Some(ip) = ip.filter(|ip| ip.is_loopback()) else {
        return check(
            NAME,
            CheckStatus::Warn,
            format!("server host {host:?} is not a loopback address; not probed"),
            Some("configure the server to bind 127.0.0.1"),
        );
    };
    let addr = SocketAddr::new(ip, port);
    match TcpStream::connect_timeout(&addr, SOCKET_TIMEOUT) {
        Ok(_) => check(
            NAME,
            CheckStatus::Ok,
            format!("accepting connections on {addr}"),
            None,
        ),
        Err(e) => check(
            NAME,
            CheckStatus::Warn,
            format!("nothing accepting connections on {addr} ({e})"),
            Some("the server may be stopped or server.json may be stale; run `tpt-lean server status`"),
        ),
    }
}

/// Start the language server for the project and shut it down again.
pub fn lsp_check(project_root: &Path) -> Check {
    const NAME: &str = "language server";
    let tc = tpt_lean_adapter::discover_toolchain();
    if tc.lean.is_none() && tc.lake.is_none() {
        return check(NAME, CheckStatus::Skipped, "lean and lake were not found", None);
    }
    let (program, args) = match server_command(project_root, tc.lake.as_ref(), tc.lean.as_ref()) {
        Ok(v) => v,
        Err(e) => return check(NAME, CheckStatus::Fail, e.to_string(), None),
    };
    match LspClient::start(&program, &args, project_root, LSP_START_TIMEOUT) {
        Ok(client) => {
            client.shutdown(LSP_SHUTDOWN_TIMEOUT);
            check(NAME, CheckStatus::Ok, "started, initialized and shut down", None)
        }
        Err(e) => check(
            NAME,
            CheckStatus::Fail,
            format!("could not start or initialize the language server: {e}"),
            Some("check that `lake serve` works in the project"),
        ),
    }
}

/// Warn when entries in the global cache directory are older than `max_age_days`.
///
/// Only the top level of `cache_dir` is inspected, and symbolic links are not followed.
pub fn cache_age_check(cache_dir: Option<&Path>, now: SystemTime, max_age_days: u64) -> Check {
    const NAME: &str = "cache age";
    let Some(dir) = cache_dir else {
        return check(
            NAME,
            CheckStatus::Skipped,
            "no global cache directory could be determined",
            None,
        );
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return check(
            NAME,
            CheckStatus::Skipped,
            format!("{} does not exist yet", dir.display()),
            None,
        );
    };
    let max_age = Duration::from_secs(max_age_days.saturating_mul(24 * 60 * 60));
    let mut total = 0u64;
    let mut old = 0u64;
    let mut oldest = Duration::ZERO;
    for entry in entries.flatten().take(100_000) {
        total += 1;
        let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        let Ok(modified) = meta.modified() else { continue };
        let age = now.duration_since(modified).unwrap_or(Duration::ZERO);
        if age > max_age {
            old += 1;
            oldest = oldest.max(age);
        }
    }
    if old == 0 {
        return check(
            NAME,
            CheckStatus::Ok,
            format!("{total} cache entries, none older than {max_age_days} days"),
            None,
        );
    }
    check(
        NAME,
        CheckStatus::Warn,
        format!(
            "{old} of {total} cache entries are older than {max_age_days} days (oldest {} days)",
            oldest.as_secs() / 86_400
        ),
        Some("run `tpt-lean cache prune`"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::net::TcpListener;

    fn temp(tag: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("tpt-tools-doc-{tag}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn server_check_skips_without_config() {
        let dir = temp("nosrv");
        assert_eq!(server_socket_check(&dir).status, CheckStatus::Skipped);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn server_check_probes_loopback_port() {
        let dir = temp("srv");
        fs::create_dir_all(dir.join(".tpt-lean")).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        fs::write(
            dir.join(".tpt-lean").join("server.json"),
            format!(r#"{{"port": {port}}}"#),
        )
        .unwrap();
        assert_eq!(server_socket_check(&dir).status, CheckStatus::Ok);

        // Port 1 is never bound by a normal user process. Re-probing the port that was just
        // released is racy: other processes can bind it, and the probe then succeeds.
        drop(listener);
        fs::write(dir.join(".tpt-lean").join("server.json"), r#"{"port": 1}"#).unwrap();
        let c = server_socket_check(&dir);
        assert_eq!(c.status, CheckStatus::Warn, "{c:?}");

        fs::write(
            dir.join(".tpt-lean").join("server.json"),
            r#"{"port": 9, "host": "10.1.2.3"}"#,
        )
        .unwrap();
        let c = server_socket_check(&dir);
        assert_eq!(c.status, CheckStatus::Warn);
        assert!(c.detail.contains("not a loopback"));

        fs::write(dir.join(".tpt-lean").join("server.json"), "not json").unwrap();
        assert_eq!(server_socket_check(&dir).status, CheckStatus::Fail);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_age_warns_on_old_entries() {
        let dir = temp("cache");
        fs::write(dir.join("entry"), b"x").unwrap();
        let far_future = SystemTime::now() + Duration::from_secs(60 * 24 * 60 * 60);
        let c = cache_age_check(Some(&dir), far_future, 30);
        assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
        assert_eq!(c.suggestion.as_deref(), Some("run `tpt-lean cache prune`"));

        let c = cache_age_check(Some(&dir), SystemTime::now(), 30);
        assert_eq!(c.status, CheckStatus::Ok);

        assert_eq!(
            cache_age_check(None, SystemTime::now(), 30).status,
            CheckStatus::Skipped
        );
        assert_eq!(
            cache_age_check(Some(&dir.join("missing")), SystemTime::now(), 30).status,
            CheckStatus::Skipped
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn extensions_return_three_checks() {
        let dir = temp("all");
        let checks = doctor_extensions(&dir);
        assert_eq!(checks.len(), 3);
        assert_eq!(checks[0].name, "server socket");
        let _ = fs::remove_dir_all(&dir);
    }
}
