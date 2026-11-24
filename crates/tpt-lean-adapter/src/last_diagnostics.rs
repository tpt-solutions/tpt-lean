// SPDX-License-Identifier: MIT OR Apache-2.0
//! The most recent diagnostic set of a project, stored in `.tpt-lean/last-diagnostics.json` so
//! that `explain --last-error` can read it without running Lean again.

use crate::diagnostics::Diagnostic;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tpt_lean_core::dirs::ProjectDirs;
use tpt_lean_core::{Error, ErrorCode, Result};

/// Version of the stored JSON layout. Bump when fields change meaning.
pub const LAST_DIAGNOSTICS_SCHEMA: u32 = 1;

const FILE_NAME: &str = "last-diagnostics.json";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LastDiagnostics {
    pub schema_version: u32,
    /// Seconds since the Unix epoch when the set was recorded.
    pub recorded_at_unix: u64,
    /// What produced the diagnostics, for example `lean <file>` or `lake build Main`.
    pub source: String,
    pub diagnostics: Vec<Diagnostic>,
}

/// Path of the stored diagnostic set for a project.
pub fn last_diagnostics_path(project_root: &Path) -> PathBuf {
    ProjectDirs::new(project_root).root.join(FILE_NAME)
}

/// Record `diagnostics` as the latest set for the project. Written atomically.
pub fn save_last_diagnostics(
    project_root: &Path,
    source: &str,
    diagnostics: &[Diagnostic],
) -> Result<PathBuf> {
    let path = last_diagnostics_path(project_root);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let record = LastDiagnostics {
        schema_version: LAST_DIAGNOSTICS_SCHEMA,
        recorded_at_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        source: source.to_string(),
        diagnostics: diagnostics.to_vec(),
    };
    let body = serde_json::to_string_pretty(&record)
        .map_err(|e| Error::new(ErrorCode::Internal, format!("cannot encode diagnostics: {e}")))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Load the latest stored set. `Ok(None)` when nothing has been recorded yet.
pub fn load_last_diagnostics(project_root: &Path) -> Result<Option<LastDiagnostics>> {
    let path = last_diagnostics_path(project_root);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let record: LastDiagnostics = serde_json::from_str(&text).map_err(|e| {
        Error::new(
            ErrorCode::Internal,
            format!("stored diagnostics are unreadable: {e}"),
        )
        .with_path(&path)
    })?;
    if record.schema_version != LAST_DIAGNOSTICS_SCHEMA {
        return Err(Error::new(
            ErrorCode::Internal,
            format!(
                "stored diagnostics have schema {}, expected {LAST_DIAGNOSTICS_SCHEMA}",
                record.schema_version
            ),
        )
        .with_path(&path));
    }
    Ok(Some(record))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::Severity;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tpt-lastdiag-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn round_trip_and_missing() {
        let root = temp_root("roundtrip");
        assert!(load_last_diagnostics(&root).unwrap().is_none());
        let diags = vec![Diagnostic {
            file: "A.lean".into(),
            line: 3,
            column: 4,
            severity: Severity::Error,
            message: "unknown identifier".into(),
        }];
        save_last_diagnostics(&root, "lean A.lean", &diags).unwrap();
        let loaded = load_last_diagnostics(&root).unwrap().expect("stored");
        assert_eq!(loaded.diagnostics, diags);
        assert_eq!(loaded.source, "lean A.lean");
        assert_eq!(loaded.schema_version, LAST_DIAGNOSTICS_SCHEMA);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn corrupt_file_is_reported() {
        let root = temp_root("corrupt");
        let path = last_diagnostics_path(&root);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{not json").unwrap();
        assert!(load_last_diagnostics(&root).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
