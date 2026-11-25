// SPDX-License-Identifier: MIT OR Apache-2.0
//! `check.project`: build the whole Lake project and return the CI report (status, diagnostics,
//! failure summaries, cache counters). It runs `lake build`, so it needs the build permission.
//! It blocks until the build finishes or times out; `build.run` is the background variant.

use serde_json::Value;
use tpt_lean_adapter::discover_toolchain;
use tpt_lean_tools::ci::{run_ci, CiOptions};

use crate::{app_error, MethodResult, RpcError, Server, INVALID_PARAMS};

impl Server {
    pub(crate) fn check_project(&self, params: &Value) -> MethodResult {
        let timeout_seconds = match params.get("timeout") {
            None | Some(Value::Null) => self.config().security.command_timeout_seconds,
            Some(v) => v
                .as_u64()
                .filter(|n| (1..=3600).contains(n))
                .ok_or_else(|| RpcError::new(INVALID_PARAMS, "`timeout` must be seconds from 1 to 3600"))?,
        };
        let toolchain = discover_toolchain();
        let report = run_ci(self.root(), &toolchain, &CiOptions { timeout_seconds }).map_err(app_error)?;
        serde_json::to_value(&report).map_err(|e| {
            app_error(tpt_lean_core::Error::new(
                tpt_lean_core::ErrorCode::Internal,
                e.to_string(),
            ))
        })
    }
}
