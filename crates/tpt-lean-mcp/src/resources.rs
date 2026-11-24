// SPDX-License-Identifier: MIT OR Apache-2.0
//! Read-only resources (specification section 11.5).
//!
//! Static: `tpt-lean://project/info`, `tpt-lean://project/diagnostics`,
//! `tpt-lean://index/status`. Template: `tpt-lean://file/{path}/symbols`.

use crate::{RpcError, Server, INTERNAL_ERROR, RESOURCE_NOT_FOUND};
use serde_json::{json, Value};
use tpt_lean_adapter as adapter;
use tpt_lean_core::dirs::ProjectDirs;

const PROJECT_INFO: &str = "tpt-lean://project/info";
const PROJECT_DIAGNOSTICS: &str = "tpt-lean://project/diagnostics";
const INDEX_STATUS: &str = "tpt-lean://index/status";
const FILE_PREFIX: &str = "tpt-lean://file/";
const FILE_SUFFIX: &str = "/symbols";

/// `resources/list`.
pub(crate) fn list() -> Value {
    json!({
        "resources": [
            {
                "uri": PROJECT_INFO,
                "name": "Project info",
                "description": "Project name, root, Lean version, toolchain location, and the security settings in effect.",
                "mimeType": "application/json"
            },
            {
                "uri": PROJECT_DIAGNOSTICS,
                "name": "Cached diagnostics",
                "description": "Diagnostics from the latest check and build runs, per file. Empty until a run has happened.",
                "mimeType": "application/json"
            },
            {
                "uri": INDEX_STATUS,
                "name": "Index status",
                "description": "Index schema version, sizes, packages, and integrity.",
                "mimeType": "application/json"
            }
        ]
    })
}

/// `resources/templates/list`.
pub(crate) fn templates() -> Value {
    json!({
        "resourceTemplates": [
            {
                "uriTemplate": "tpt-lean://file/{path}/symbols",
                "name": "File symbols",
                "description": "Top-level declarations in one .lean file. The path is percent-encoded and workspace-relative.",
                "mimeType": "application/json"
            }
        ]
    })
}

/// `resources/read`.
pub(crate) fn read(server: &Server, uri: &str) -> Result<Value, RpcError> {
    let body = match uri {
        PROJECT_INFO => project_info(server),
        PROJECT_DIAGNOSTICS => Ok(project_diagnostics(server)),
        INDEX_STATUS => server.index_status().map_err(|e| (INTERNAL_ERROR, e)),
        _ => match file_symbols_path(uri) {
            Some(path) => file_symbols(server, &path),
            None => return Err((RESOURCE_NOT_FOUND, format!("unknown resource {uri}"))),
        },
    }?;
    Ok(json!({
        "contents": [{ "uri": uri, "mimeType": "application/json", "text": body.to_string() }]
    }))
}

fn project_info(server: &Server) -> Result<Value, RpcError> {
    let tc = adapter::discover_toolchain();
    let config = server.config();
    let index_present = ProjectDirs::new(server.root()).index_db().exists();
    Ok(json!({
        "name": config.project.name,
        "root": server.root().display().to_string().replace('\\', "/"),
        "lean_version": config.project.lean_version,
        "toolchain": {
            "lean": tc.lean.map(|p| p.display().to_string()),
            "lake": tc.lake.map(|p| p.display().to_string()),
        },
        "index_present": index_present,
        "security": {
            "workspace_only": config.security.workspace_only,
            "allow_write": config.security.allow_write,
            "allow_build": config.security.allow_build,
            "command_timeout_seconds": config.security.command_timeout_seconds,
        },
    }))
}

fn project_diagnostics(server: &Server) -> Value {
    let files: Vec<Value> = server
        .cached_diagnostics()
        .into_iter()
        .map(|(file, diagnostics)| json!({ "file": file, "diagnostics": diagnostics }))
        .collect();
    json!({ "files": files })
}

/// The `{path}` of a `tpt-lean://file/{path}/symbols` URI, percent-decoded.
pub(crate) fn file_symbols_path(uri: &str) -> Option<String> {
    let inner = uri.strip_prefix(FILE_PREFIX)?.strip_suffix(FILE_SUFFIX)?;
    if inner.is_empty() {
        return None;
    }
    percent_decode(inner)
}

fn file_symbols(server: &Server, path: &str) -> Result<Value, RpcError> {
    let (_, rel, text) = server.lean_file(path).map_err(|e| (RESOURCE_NOT_FOUND, e))?;
    let parse = tpt_lean_syntax::parse(&text);
    let symbols: Vec<Value> = parse
        .symbols
        .iter()
        .map(|s| {
            json!({
                "name": s.name,
                "short_name": s.short_name,
                "kind": s.kind,
                "namespace": s.namespace,
                "line": s.line,
                "docstring": s.docstring,
                "is_private": s.is_private,
            })
        })
        .collect();
    Ok(json!({ "file": rel, "symbols": symbols }))
}

/// Decode `%XX` escapes. Returns `None` for a malformed escape or non-UTF-8 output.
pub(crate) fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_symbols_uri_is_parsed_and_decoded() {
        assert_eq!(
            file_symbols_path("tpt-lean://file/src/Main.lean/symbols").as_deref(),
            Some("src/Main.lean")
        );
        assert_eq!(
            file_symbols_path("tpt-lean://file/src%2FMain.lean/symbols").as_deref(),
            Some("src/Main.lean")
        );
        assert!(file_symbols_path("tpt-lean://file//symbols").is_none());
        assert!(file_symbols_path("tpt-lean://file/a.lean").is_none());
        assert!(file_symbols_path("tpt-lean://file/a%2/symbols").is_none());
    }

    #[test]
    fn percent_decoding_rejects_bad_escapes() {
        assert_eq!(percent_decode("a%20b").as_deref(), Some("a b"));
        assert_eq!(percent_decode("a%zz"), None);
        assert_eq!(percent_decode("%FF"), None);
    }

    #[test]
    fn list_names_the_three_static_resources() {
        let l = list();
        let uris: Vec<&str> = l["resources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["uri"].as_str().unwrap())
            .collect();
        assert_eq!(uris, vec![PROJECT_INFO, PROJECT_DIAGNOSTICS, INDEX_STATUS]);
        assert_eq!(
            templates()["resourceTemplates"][0]["uriTemplate"],
            "tpt-lean://file/{path}/symbols"
        );
    }

    fn temp_project(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tpt-mcp-res-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Sub")).unwrap();
        dir
    }

    #[test]
    fn project_diagnostics_is_empty_until_a_run_caches_some() {
        let dir = temp_project("diag");
        let s = Server::new(&dir).unwrap();
        let r = read(&s, PROJECT_DIAGNOSTICS).unwrap();
        let body: Value = serde_json::from_str(r["contents"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(body["files"], json!([]));
        s.store_diagnostics(
            "A.lean",
            vec![json!({"file":"A.lean","line":1,"column":1,"severity":"error","message":"m"})],
        );
        let r = read(&s, PROJECT_DIAGNOSTICS).unwrap();
        let body: Value = serde_json::from_str(r["contents"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(body["files"][0]["file"], "A.lean");
    }

    #[test]
    fn project_info_reports_security_settings() {
        let dir = temp_project("info");
        let s = Server::new(&dir).unwrap();
        let r = read(&s, PROJECT_INFO).unwrap();
        let body: Value = serde_json::from_str(r["contents"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(body["security"]["allow_write"], false);
        assert_eq!(body["security"]["allow_build"], true);
        assert_eq!(body["index_present"], false);
    }

    #[test]
    fn file_symbols_resource_lists_declarations_and_checks_path() {
        let dir = temp_project("syms");
        std::fs::write(
            dir.join("Sub").join("M.lean"),
            "namespace N\ndef f := 1\ntheorem t : True := trivial\nend N\n",
        )
        .unwrap();
        let s = Server::new(&dir).unwrap();
        let r = read(&s, "tpt-lean://file/Sub%2FM.lean/symbols").unwrap();
        let body: Value = serde_json::from_str(r["contents"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(body["file"], "Sub/M.lean");
        let names: Vec<&str> = body["symbols"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["name"].as_str().unwrap())
            .collect();
        assert!(names.iter().any(|n| n.ends_with("f")));
        assert!(names.iter().any(|n| n.ends_with("t")));

        let escape = read(&s, "tpt-lean://file/..%2F..%2Fetc%2Fpasswd.lean/symbols");
        assert_eq!(escape.unwrap_err().0, RESOURCE_NOT_FOUND);
        let not_lean = read(&s, "tpt-lean://file/Sub%2FM.txt/symbols");
        assert_eq!(not_lean.unwrap_err().0, RESOURCE_NOT_FOUND);
    }

    #[test]
    fn unknown_resource_is_not_found() {
        let dir = temp_project("unknown");
        let s = Server::new(&dir).unwrap();
        assert_eq!(read(&s, "tpt-lean://nope").unwrap_err().0, RESOURCE_NOT_FOUND);
    }
}
