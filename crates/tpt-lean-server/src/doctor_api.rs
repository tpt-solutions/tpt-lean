// SPDX-License-Identifier: MIT OR Apache-2.0
//! `project.doctor`: the same health checks as `tpt-lean doctor`. Some checks start the Lean
//! language server, so the method needs the build permission. The write check creates the
//! `.tpt-lean` data directory when it is missing, as the CLI does.

use serde_json::{json, Value};
use tpt_lean_tools::project_doctor::project_checks;

use crate::{MethodResult, Server};

impl Server {
    pub(crate) fn project_doctor(&self, _params: &Value) -> MethodResult {
        // The server only starts with a configuration that loads, so the config check passes.
        let checks = project_checks(self.root(), Ok(()));
        let ok = !checks.iter().any(|c| c.status == "fail");
        Ok(json!({ "ok": ok, "checks": checks }))
    }
}
