// SPDX-License-Identifier: MIT OR Apache-2.0
//! `context.pack`: a token-budgeted bundle around one position (snippet, goal, hypotheses,
//! diagnostics, imports, nearby declarations, lemma candidates). It starts the Lean language
//! server, so it needs the build permission. The same bundle as `tpt-lean context-pack`.

use std::time::Duration;

use serde_json::Value;
use tpt_lean_adapter::context::{assemble_pack, PackRequest};
use tpt_lean_adapter::explain::snapshot_in_session;

use crate::{app_error, position_param, str_param, MethodResult, RpcError, Server, INVALID_PARAMS};

/// Default token budget, matching the CLI.
const DEFAULT_MAX_TOKENS: u64 = 6000;

impl Server {
    pub(crate) fn context_pack(&self, params: &Value) -> MethodResult {
        let rel = str_param(params, "path")?;
        let path = self.lean_path_param(params, "path")?;
        if !path.is_file() {
            return Err(RpcError::new(INVALID_PARAMS, format!("no such file: {rel}")));
        }
        let line = position_param(params, "line")?;
        let column = position_param(params, "column")?;
        let max_tokens = match params.get("max_tokens") {
            None | Some(Value::Null) => DEFAULT_MAX_TOKENS,
            Some(v) => v.as_u64().filter(|n| (1..=100_000).contains(n)).ok_or_else(|| {
                RpcError::new(INVALID_PARAMS, "`max_tokens` must be an integer from 1 to 100000")
            })?,
        } as usize;
        let text = std::fs::read_to_string(&path)
            .map_err(|e| app_error(tpt_lean_core::Error::from(e).with_path(&path)))?;
        let timeout = self.config().security.command_timeout_seconds;
        let req = PackRequest {
            root: self.root(),
            path: &path,
            label: &rel,
            text: &text,
            line,
            column,
            max_tokens,
            timeout: Duration::from_secs(timeout),
            command_timeout_seconds: timeout,
        };
        let snap = self
            .with_warm_session(|session, version| {
                snapshot_in_session(session, &path, &text, line, column, version, req.timeout)
            })
            .map_err(app_error)?;
        let pack = assemble_pack(snap, &req).map_err(app_error)?;
        serde_json::to_value(&pack).map_err(|e| {
            app_error(tpt_lean_core::Error::new(
                tpt_lean_core::ErrorCode::Internal,
                e.to_string(),
            ))
        })
    }
}
