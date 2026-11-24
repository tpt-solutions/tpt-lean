// SPDX-License-Identifier: MIT OR Apache-2.0
//! JSON output conventions.
//!
//! Every machine-readable response is wrapped in an envelope with a schema version so clients
//! can detect incompatible changes. Field names are stable: add fields, never rename them.

use serde::Serialize;

/// Current version of the JSON output schema.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Serialize)]
pub struct Envelope<'a, T: Serialize> {
    pub schema_version: u32,
    pub tool: &'static str,
    pub kind: &'a str,
    pub ok: bool,
    pub data: T,
}

impl<'a, T: Serialize> Envelope<'a, T> {
    pub fn ok(kind: &'a str, data: T) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            tool: "tpt-lean",
            kind,
            ok: true,
            data,
        }
    }
}

/// Error body used in JSON output on failure.
#[derive(Debug, Serialize)]
pub struct ErrorBody {
    pub code: &'static str,
    pub message: String,
    pub path: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ErrorEnvelope {
    pub schema_version: u32,
    pub tool: &'static str,
    pub kind: &'static str,
    pub ok: bool,
    pub error: ErrorBody,
}

impl ErrorEnvelope {
    pub fn new(error: &crate::error::Error) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            tool: "tpt-lean",
            kind: "error",
            ok: false,
            error: ErrorBody {
                code: error.code().as_str(),
                message: error.message().to_string(),
                path: error.path().map(|p| p.display().to_string()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_shape() {
        let v = serde_json::to_value(Envelope::ok("demo", 1)).unwrap();
        assert_eq!(v["schema_version"], 1);
        assert_eq!(v["kind"], "demo");
        assert_eq!(v["ok"], true);
        assert_eq!(v["data"], 1);
    }
}
