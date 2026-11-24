// SPDX-License-Identifier: MIT OR Apache-2.0
//! Method handlers. Permission checks happen in `Server::handle` before these run; path
//! parameters are resolved through `WorkspaceRoot` inside `lean_path_param`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};
use tpt_lean_adapter::lsp::{self, LspClient};
use tpt_lean_adapter::{discover_toolchain, execute, lean_policy, Command, Severity};
use tpt_lean_core::dirs::ProjectDirs;
use tpt_lean_core::policy::CancelToken;
use tpt_lean_core::Result as CoreResult;
use tpt_lean_index::{default_scan_roots, Index};
use tpt_lean_search::SearchOptions;
use tpt_lean_syntax::position::LineIndex;

use crate::{
    app_error, limit_param, lock, position_param, str_param, MethodResult, RpcError, Server, APP_ERROR,
    INVALID_PARAMS,
};

impl Server {
    pub(crate) fn search_query(&self, params: &Value) -> MethodResult {
        let query = str_param(params, "query")?;
        let limit = limit_param(params, 10, 200)?;
        let index = self.open_index()?;
        let opts = SearchOptions {
            limit,
            ..Default::default()
        };
        let hits = tpt_lean_search::search(&index, &query, &opts).map_err(app_error)?;
        let items: Vec<Value> = hits
            .into_iter()
            .map(|h| {
                json!({
                    "name": h.name,
                    "kind": h.kind,
                    "source": h.file,
                    "line": h.line,
                    "score": (h.score * 1000.0).round() / 1000.0,
                })
            })
            .collect();
        Ok(json!({ "items": items }))
    }

    pub(crate) fn search_symbol(&self, params: &Value) -> MethodResult {
        let name = str_param(params, "name")?;
        let limit = limit_param(params, 10, 200)?;
        let index = self.open_index()?;
        let rows = index.symbols_named(&name, limit).map_err(app_error)?;
        let items: Vec<Value> = rows
            .into_iter()
            .map(|s| {
                json!({
                    "name": s.name,
                    "short_name": s.short_name,
                    "kind": s.kind,
                    "namespace": s.namespace,
                    "package": s.package,
                    "source": s.file,
                    "line": s.line,
                })
            })
            .collect();
        Ok(json!({ "items": items }))
    }

    pub(crate) fn format_text(&self, params: &Value) -> MethodResult {
        let text = str_param(params, "text")?;
        let out = tpt_lean_fmt::format_text(&text).map_err(|e| RpcError::new(APP_ERROR, e.to_string()))?;
        Ok(json!({ "formatted": out.output, "changed": out.changed }))
    }

    /// Format a file in place. Requires the `write` permission (checked by the dispatcher).
    /// Refuses files whose syntax is uncertain; writes through a temporary file and a rename.
    pub(crate) fn format_file(&self, params: &Value) -> MethodResult {
        let rel = str_param(params, "path")?;
        let path = self.lean_path_param(params, "path")?;
        let text = std::fs::read_to_string(&path)
            .map_err(|e| RpcError::new(APP_ERROR, format!("cannot read {rel}: {e}")))?;
        let out = tpt_lean_fmt::format_text(&text)
            .map_err(|e| RpcError::new(APP_ERROR, format!("refusing to format {rel}: {e}")))?;
        let written = out.changed;
        if written {
            write_atomic(&path, out.output.as_bytes())?;
        }
        Ok(json!({ "path": rel, "changed": out.changed, "written": written }))
    }

    /// Refresh the index in `.tpt-lean/`. Requires the `write` permission.
    pub(crate) fn index_refresh(&self) -> MethodResult {
        let dirs = ProjectDirs::new(self.root());
        dirs.ensure()
            .map_err(|e| RpcError::new(APP_ERROR, format!("cannot create .tpt-lean: {e}")))?;
        let mut index = Index::open(&dirs.index_db()).map_err(app_error)?;
        let roots = default_scan_roots(self.root(), self.config().index.mathlib);
        let stats = index.refresh(&roots).map_err(app_error)?;
        Ok(json!({
            "scanned": stats.scanned,
            "updated": stats.updated,
            "unchanged": stats.unchanged,
            "removed": stats.removed,
        }))
    }

    /// Run `lean` on one file. Requires the `build` permission. Results are kept in memory for
    /// `diagnostics.get`.
    pub(crate) fn check_file(&self, params: &Value) -> MethodResult {
        let rel = str_param(params, "path")?;
        let path = self.lean_path_param(params, "path")?;
        if !path.is_file() {
            return Err(RpcError::new(INVALID_PARAMS, format!("no such file: {rel}")));
        }
        let toolchain = discover_toolchain();
        let policy = lean_policy(self.config().security.command_timeout_seconds);
        let command = Command::LeanCheck { file: path.clone() };
        let result = execute(
            &toolchain,
            &command,
            Some(self.root()),
            &policy,
            &CancelToken::new(),
        )
        .map_err(app_error)?;
        let diagnostics = result.diagnostics;
        let ok = result.output.success() && !diagnostics.iter().any(|d| d.severity == Severity::Error);
        lock(&self.diagnostics).insert(path, diagnostics.clone());
        Ok(json!({
            "path": rel,
            "ok": ok,
            "exit_code": result.output.exit_code,
            "timed_out": result.output.timed_out,
            "output_truncated": result.output.output_truncated,
            "diagnostics": diagnostics,
        }))
    }

    /// Diagnostics from the last `check.file` for this file in this server process.
    pub(crate) fn diagnostics_get(&self, params: &Value) -> MethodResult {
        let rel = str_param(params, "path")?;
        let path = self.lean_path_param(params, "path")?;
        let known = lock(&self.diagnostics).get(&path).cloned();
        Ok(json!({
            "path": rel,
            "known": known.is_some(),
            "diagnostics": known.unwrap_or_default(),
        }))
    }

    /// Plain goals at a 1-based line and column, from the Lean language server. Requires the
    /// `build` permission because it starts a Lean process.
    pub(crate) fn goals_at(&self, params: &Value) -> MethodResult {
        let rel = str_param(params, "path")?;
        let path = self.lean_path_param(params, "path")?;
        let line = position_param(params, "line")?;
        let column = position_param(params, "column")?;
        let text = std::fs::read_to_string(&path)
            .map_err(|e| RpcError::new(APP_ERROR, format!("cannot read {rel}: {e}")))?;
        let index = LineIndex::new(&text);
        let offset = index.offset_of(line, column).ok_or_else(|| {
            RpcError::new(
                INVALID_PARAMS,
                format!("{rel}:{line}:{column} is outside the file"),
            )
        })?;
        let utf16 = index.position(offset).column_utf16 as u32;

        let timeout = Duration::from_secs(self.config().security.command_timeout_seconds);
        let toolchain = discover_toolchain();
        let (program, args) =
            lsp::server_command(self.root(), toolchain.lake.as_ref(), toolchain.lean.as_ref())
                .map_err(app_error)?;
        let mut client = LspClient::start(&program, &args, self.root(), timeout).map_err(app_error)?;
        let goals = lsp_goals(&mut client, &path, &text, (line - 1) as u32, utf16, timeout);
        client.shutdown(Duration::from_secs(5));
        let goals = goals.map_err(|e| RpcError::new(APP_ERROR, e.to_string()))?;
        Ok(json!({ "path": rel, "line": line, "column": column, "goals": goals }))
    }
}

fn lsp_goals(
    client: &mut LspClient,
    path: &Path,
    text: &str,
    line0: u32,
    character_utf16: u32,
    timeout: Duration,
) -> CoreResult<Vec<String>> {
    let uri = lsp::open_document(client, path, text)?;
    lsp::plain_goals(client, &uri, line0, character_utf16, timeout)
}

/// Write `bytes` to `path` via a temporary sibling file and a rename, so readers never see a
/// partial file.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), RpcError> {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp: PathBuf = path.with_file_name(format!(".{name}.tpt-lean-tmp"));
    let result = std::fs::write(&tmp, bytes).and_then(|_| std::fs::rename(&tmp, path));
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(RpcError::new(APP_ERROR, format!("cannot write file: {e}")));
    }
    Ok(())
}
