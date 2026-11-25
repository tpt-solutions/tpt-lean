// SPDX-License-Identifier: MIT OR Apache-2.0
//! `diagnostics.explain`: the error Lean reports at one position, the goal there, and lemma
//! candidates from the project index. It starts the Lean language server, so it needs the build
//! permission. The same explanation as `tpt-lean explain FILE:LINE:COL`.

use std::time::Duration;

use serde_json::{json, Value};
use tpt_lean_adapter::explain::{error_at, lemma_candidates, snapshot_at, split_goal};
use tpt_lean_core::dirs::ProjectDirs;

use crate::{app_error, position_param, str_param, MethodResult, RpcError, Server, INVALID_PARAMS};

impl Server {
    pub(crate) fn diagnostics_explain(&self, params: &Value) -> MethodResult {
        let rel = str_param(params, "path")?;
        let path = self.lean_path_param(params, "path")?;
        if !path.is_file() {
            return Err(RpcError::new(INVALID_PARAMS, format!("no such file: {rel}")));
        }
        let line = position_param(params, "line")?;
        let column = position_param(params, "column")?;
        let text = std::fs::read_to_string(&path)
            .map_err(|e| app_error(tpt_lean_core::Error::from(e).with_path(&path)))?;
        let timeout = self.config().security.command_timeout_seconds;
        let snap = snapshot_at(
            self.root(),
            &path,
            &rel,
            &text,
            line,
            column,
            Duration::from_secs(timeout),
        )
        .map_err(app_error)?;
        let error = error_at(&snap.diagnostics, &rel, snap.line0, snap.utf16);
        let (hypotheses, target) = match snap.goals.first() {
            Some(g) => split_goal(g),
            None => (Vec::new(), None),
        };
        let suggestions = target
            .as_deref()
            .map(|t| lemma_candidates(self.root(), t, 5))
            .unwrap_or_default();
        Ok(json!({
            "error": error,
            "context": { "goal": target, "hypotheses": hypotheses },
            "suggestions": suggestions,
        }))
    }
}

impl Server {
    /// `search.goal`: lemma candidates for the goal at a position. The goal's target is searched
    /// in the index, the same search `context.pack` uses. `limit` is 1 to 50, default 5.
    pub(crate) fn search_goal(&self, params: &Value) -> MethodResult {
        let rel = str_param(params, "path")?;
        let path = self.lean_path_param(params, "path")?;
        if !path.is_file() {
            return Err(RpcError::new(INVALID_PARAMS, format!("no such file: {rel}")));
        }
        let line = position_param(params, "line")?;
        let column = position_param(params, "column")?;
        let limit = match params.get("limit") {
            None | Some(Value::Null) => 5,
            Some(v) => v
                .as_u64()
                .filter(|n| (1..=50).contains(n))
                .ok_or_else(|| RpcError::new(INVALID_PARAMS, "`limit` must be an integer from 1 to 50"))?
                as usize,
        };
        let text = std::fs::read_to_string(&path)
            .map_err(|e| app_error(tpt_lean_core::Error::from(e).with_path(&path)))?;
        let timeout = self.config().security.command_timeout_seconds;
        let snap = snapshot_at(
            self.root(),
            &path,
            &rel,
            &text,
            line,
            column,
            Duration::from_secs(timeout),
        )
        .map_err(app_error)?;
        let (hypotheses, target) = match snap.goals.first() {
            Some(g) => split_goal(g),
            None => (Vec::new(), None),
        };
        let indexed = ProjectDirs::new(self.root()).index_db().exists();
        let results = target
            .as_deref()
            .map(|t| lemma_candidates(self.root(), t, limit))
            .unwrap_or_default();
        Ok(json!({
            "goal": target,
            "hypotheses": hypotheses,
            "indexed": indexed,
            "results": results,
        }))
    }
}
