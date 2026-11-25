// SPDX-License-Identifier: MIT OR Apache-2.0
//! Import methods: `imports.suggest` finds modules that declare a name, from the local index, and
//! `imports.unused` lists imports a file does not use. Both only read the project.

use serde_json::{json, Value};

use crate::{app_error, str_param, MethodResult, RpcError, Server, INVALID_PARAMS};

impl Server {
    /// Modules that declare names matching `query`, from the index (run `index.refresh` first).
    /// `limit` is 1 to 50, default 10.
    pub(crate) fn imports_suggest(&self, params: &Value) -> MethodResult {
        let query = str_param(params, "query")?;
        let limit = match params.get("limit") {
            None | Some(Value::Null) => 10,
            Some(v) => v
                .as_u64()
                .filter(|n| (1..=50).contains(n))
                .ok_or_else(|| RpcError::new(INVALID_PARAMS, "`limit` must be an integer from 1 to 50"))?
                as usize,
        };
        let suggestions =
            tpt_lean_tools::imports::suggest_imports(self.root(), &query, limit).map_err(app_error)?;
        Ok(json!({ "suggestions": suggestions }))
    }

    /// Imports in one `.lean` file that nothing in the file uses.
    pub(crate) fn imports_unused(&self, params: &Value) -> MethodResult {
        let path = self.lean_path_param(params, "path")?;
        if !path.is_file() {
            let rel = str_param(params, "path")?;
            return Err(RpcError::new(INVALID_PARAMS, format!("no such file: {rel}")));
        }
        let findings = tpt_lean_tools::imports::unused_imports(self.root(), &path).map_err(app_error)?;
        Ok(json!({ "findings": findings }))
    }
}
