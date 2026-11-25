// SPDX-License-Identifier: MIT OR Apache-2.0
//! Workspace symbols answered from the project index. The index is only read here; it is never
//! created or refreshed by the proxy.

use crate::symbols::symbol_kind;
use serde_json::{json, Value};
use std::path::Path;
use tpt_lean_adapter::lsp::path_to_uri;
use tpt_lean_core::Result;
use tpt_lean_index::{Index, SymbolRow};

/// `SymbolInformation` entries whose name matches `query`. Empty when no index exists yet.
pub fn workspace_symbols(
    project_root: &Path,
    index_path: &Path,
    query: &str,
    limit: usize,
) -> Result<Vec<Value>> {
    if !index_path.is_file() {
        return Ok(Vec::new());
    }
    let index = Index::open(index_path)?;
    let rows = index.symbols_named(query, limit)?;
    Ok(rows
        .iter()
        .map(|row| symbol_information(project_root, row))
        .collect())
}

/// Declarations whose full name or short name is exactly `name`, as LSP `Location`s. Used for
/// go-to-definition. Empty when no index exists or nothing matches.
pub fn definitions(
    project_root: &Path,
    index_path: &Path,
    name: &str,
    limit: usize,
) -> Result<Vec<Value>> {
    if !index_path.is_file() {
        return Ok(Vec::new());
    }
    let index = Index::open(index_path)?;
    let rows = index.symbols_named(name, limit.max(1) * 10)?;
    Ok(rows
        .iter()
        .filter(|row| row.name == name || row.short_name.as_deref() == Some(name))
        .take(limit)
        .map(|row| location(project_root, row))
        .collect())
}

/// The file and line of a declaration, as an LSP `Location` with an empty range there.
fn location(project_root: &Path, row: &SymbolRow) -> Value {
    let root = if row.package == "project" {
        project_root.to_path_buf()
    } else {
        project_root.join(".lake").join("packages").join(&row.package)
    };
    let path = root.join(&row.file);
    let line = (row.line.max(1) - 1) as u32;
    let position = json!({ "line": line, "character": 0 });
    json!({
        "uri": path_to_uri(&path),
        "range": { "start": position, "end": position }
    })
}

fn symbol_information(project_root: &Path, row: &SymbolRow) -> Value {
    json!({
        "name": row.short_name.clone().unwrap_or_else(|| row.name.clone()),
        "kind": symbol_kind(&row.kind),
        "containerName": row.namespace,
        "location": location(project_root, row),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_index_gives_no_symbols() {
        let dir = std::env::temp_dir().join(format!("tpt-lsp-ws-none-{}", std::process::id()));
        let symbols = workspace_symbols(&dir, &dir.join("index.sqlite"), "anything", 10).unwrap();
        assert!(symbols.is_empty());
    }

    #[test]
    fn definitions_match_the_exact_name_only() {
        let dir = std::env::temp_dir().join(format!("tpt-lsp-def-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Lib.lean"), "namespace Geo
theorem compact_ok : True := trivial
end Geo
").unwrap();
        let db = dir.join("index.sqlite");
        let mut index = Index::open(&db).unwrap();
        index.refresh(&tpt_lean_index::default_scan_roots(&dir, false)).unwrap();
        drop(index);
        let by_full = definitions(&dir, &db, "Geo.compact_ok", 5).unwrap();
        let by_short = definitions(&dir, &db, "compact_ok", 5).unwrap();
        let partial = definitions(&dir, &db, "compact", 5).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(by_full.len(), 1, "{by_full:?}");
        assert_eq!(by_short.len(), 1);
        assert_eq!(by_full[0]["range"]["start"]["line"], 1);
        assert!(partial.is_empty());
    }

    #[test]
    fn indexed_declarations_are_found() {
        let dir = std::env::temp_dir().join(format!("tpt-lsp-ws-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Lib.lean"), "theorem widget_size_ok : True := trivial\n").unwrap();
        let db = dir.join("index.sqlite");
        let mut index = Index::open(&db).unwrap();
        index
            .refresh(&tpt_lean_index::default_scan_roots(&dir, false))
            .unwrap();
        drop(index);
        let symbols = workspace_symbols(&dir, &db, "widget_size", 10).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(symbols.len(), 1, "{symbols:?}");
        assert_eq!(symbols[0]["name"], "widget_size_ok");
        assert_eq!(symbols[0]["location"]["range"]["start"]["line"], 0);
    }
}
