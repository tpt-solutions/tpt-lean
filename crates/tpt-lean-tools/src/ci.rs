// SPDX-License-Identifier: MIT OR Apache-2.0
//! CI runner: runs `lake build` for a project through the adapter and produces a machine-readable
//! report, plus a SARIF 2.1.0 document for code-scanning tools.
//!
//! Cache statistics come from the build cache that `tpt-lean build` uses. A hit means a
//! successful build with the same key is already recorded. Lake still runs either way: the cache
//! reports state and never replaces the build, since Lean remains the source of truth. A passing
//! build is recorded, as `tpt-lean build` does.
//!
//! SARIF validation is structural only (see [`validate_sarif_shape`]). The official JSON schema is
//! not vendored, so the output is not checked against it.

use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Instant;
use tpt_lean_adapter::{Command, Diagnostic, Severity, Toolchain};
use tpt_lean_core::paths::WorkspaceRoot;
use tpt_lean_core::policy::CancelToken;
use tpt_lean_core::Result;

/// Version of the CI report layout.
pub const CI_SCHEMA_VERSION: u32 = 1;
/// Stable SARIF schema URI (OASIS, SARIF 2.1.0).
pub const SARIF_SCHEMA: &str =
    "https://docs.oasis-open.org/sarif/sarif/v2.1.0/errata01/os/schemas/sarif-schema-2.1.0.json";
pub const SARIF_VERSION: &str = "2.1.0";

#[derive(Debug, Clone)]
pub struct CiOptions {
    /// Timeout for `lake build`, in seconds.
    pub timeout_seconds: u64,
}

impl Default for CiOptions {
    fn default() -> Self {
        Self { timeout_seconds: 600 }
    }
}

/// Outcome of the build step: `passed`, `failed`, `timeout`, or `not_run`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BuildStatus {
    pub status: &'static str,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
}

/// One machine-readable failure. `kind` is `lean_error`, `build_failed` or `environment`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FailureSummary {
    pub kind: &'static str,
    pub file: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub message: String,
}

/// Build-cache counters for this run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CacheStats {
    pub available: bool,
    pub hits: u64,
    pub misses: u64,
    pub entries: u64,
    pub note: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolchainInfo {
    pub lean: Option<String>,
    pub lake: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CiReport {
    pub schema_version: u32,
    pub ok: bool,
    /// Project root as given to [`run_ci`].
    pub project: String,
    pub toolchain: ToolchainInfo,
    pub build: BuildStatus,
    pub total_ms: u64,
    pub diagnostics: Vec<Diagnostic>,
    pub failures: Vec<FailureSummary>,
    pub cache: CacheStats,
}

fn ms(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn path_text(p: &Option<PathBuf>) -> Option<String> {
    p.as_ref().map(|p| p.display().to_string())
}

/// Run `lake build` in `project_root` and summarise the result. Never writes project files.
pub fn run_ci(project_root: &Path, toolchain: &Toolchain, opts: &CiOptions) -> Result<CiReport> {
    let total = Instant::now();
    let ws = WorkspaceRoot::new(project_root)?;
    let root = ws.path().to_path_buf();
    let info = ToolchainInfo {
        lean: path_text(&toolchain.lean),
        lake: path_text(&toolchain.lake),
    };
    let probe = tpt_lean_build::probe_cache(&root, None, toolchain).ok();
    let cache = match &probe {
        Some(p) => CacheStats {
            available: p.available,
            hits: u64::from(p.hit),
            misses: u64::from(p.available && !p.hit),
            entries: p.entries,
            note: if p.available {
                "a hit means a successful build with the same key is recorded; lake still runs"
            } else {
                "no cache directory is available for this user"
            },
        },
        None => CacheStats {
            available: false,
            hits: 0,
            misses: 0,
            entries: 0,
            note: "the cache probe failed; see the failures",
        },
    };

    let started = Instant::now();
    let policy = tpt_lean_adapter::lean_policy(opts.timeout_seconds);
    let outcome = tpt_lean_adapter::execute(
        toolchain,
        &Command::LakeBuild { target: None },
        Some(&root),
        &policy,
        &CancelToken::new(),
    );

    let built_modules = match &outcome {
        Ok(r) => tpt_lean_build::progress::parse_output(&r.output.stdout)
            .iter()
            .filter(|p| matches!(p, tpt_lean_build::progress::Progress::Started { .. }))
            .count(),
        Err(_) => 0,
    };

    let (build, diagnostics, failures) = match outcome {
        Err(e) => (
            BuildStatus {
                status: "not_run",
                exit_code: None,
                duration_ms: ms(started),
            },
            Vec::new(),
            vec![FailureSummary {
                kind: "environment",
                file: None,
                line: None,
                column: None,
                message: e.to_string(),
            }],
        ),
        Ok(result) => {
            let out = &result.output;
            let status = if out.timed_out {
                "timeout"
            } else if out.success() {
                "passed"
            } else {
                "failed"
            };
            let diagnostics: Vec<Diagnostic> = result
                .diagnostics
                .into_iter()
                .map(|mut d| {
                    d.file = relativize(&root, &d.file);
                    d
                })
                .collect();
            let mut failures: Vec<FailureSummary> = diagnostics
                .iter()
                .filter(|d| d.severity == Severity::Error)
                .map(|d| FailureSummary {
                    kind: "lean_error",
                    file: Some(d.file.clone()),
                    line: Some(d.line),
                    column: Some(d.column),
                    message: d.message.clone(),
                })
                .collect();
            if status != "passed" && failures.is_empty() {
                let message = if out.timed_out {
                    format!("lake build timed out after {} s", opts.timeout_seconds)
                } else {
                    format!("lake build failed with exit code {:?}", out.exit_code)
                };
                failures.push(FailureSummary {
                    kind: "build_failed",
                    file: None,
                    line: None,
                    column: None,
                    message,
                });
            }
            (
                BuildStatus {
                    status,
                    exit_code: out.exit_code,
                    duration_ms: u64::try_from(out.duration.as_millis()).unwrap_or(u64::MAX),
                },
                diagnostics,
                failures,
            )
        }
    };

    if let (Some(p), "passed") = (&probe, build.status) {
        // A failed record write must not fail CI: the build itself passed.
        let _ = tpt_lean_build::record_success(&root, &p.digest, built_modules, build.duration_ms);
    }

    Ok(CiReport {
        schema_version: CI_SCHEMA_VERSION,
        ok: build.status == "passed",
        project: root.display().to_string(),
        toolchain: info,
        build,
        total_ms: ms(total),
        diagnostics,
        failures,
        cache,
    })
}

fn relativize(root: &Path, file: &str) -> String {
    let p = Path::new(file);
    match p.strip_prefix(root) {
        Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
        Err(_) => file.replace('\\', "/"),
    }
}

fn sarif_level(s: Severity) -> &'static str {
    match s {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "note",
    }
}

fn sarif_rule_id(s: Severity) -> &'static str {
    match s {
        Severity::Error => "lean/error",
        Severity::Warning => "lean/warning",
        Severity::Info => "lean/info",
    }
}

/// Build a SARIF 2.1.0 log from a CI report. Diagnostics with a file become results; failures
/// without a location are reported as tool execution notifications.
pub fn to_sarif(report: &CiReport) -> Value {
    let results: Vec<Value> = report
        .diagnostics
        .iter()
        .map(|d| {
            json!({
                "ruleId": sarif_rule_id(d.severity),
                "level": sarif_level(d.severity),
                "message": { "text": d.message },
                "locations": [{
                    "physicalLocation": {
                        "artifactLocation": { "uri": d.file },
                        "region": {
                            "startLine": d.line.max(1),
                            "startColumn": d.column.max(1)
                        }
                    }
                }]
            })
        })
        .collect();
    let notifications: Vec<Value> = report
        .failures
        .iter()
        .filter(|f| f.file.is_none())
        .map(|f| json!({ "level": "error", "message": { "text": f.message } }))
        .collect();
    let rules = [Severity::Error, Severity::Warning, Severity::Info]
        .iter()
        .map(|s| {
            json!({
                "id": sarif_rule_id(*s),
                "name": sarif_rule_id(*s),
                "shortDescription": { "text": format!("Lean {}", sarif_level(*s)) }
            })
        })
        .collect::<Vec<_>>();
    json!({
        "$schema": SARIF_SCHEMA,
        "version": SARIF_VERSION,
        "runs": [{
            "tool": {
                "driver": {
                    "name": "tpt-lean",
                    "version": env!("CARGO_PKG_VERSION"),
                    "rules": rules
                }
            },
            "invocations": [{
                "executionSuccessful": report.ok,
                "toolExecutionNotifications": notifications
            }],
            "results": results
        }]
    })
}

/// Structural check of a SARIF document: the keys that code-scanning consumers need.
/// This is not JSON Schema validation.
pub fn validate_sarif_shape(doc: &Value) -> std::result::Result<(), String> {
    let str_at = |v: &Value, key: &str| -> std::result::Result<String, String> {
        v.get(key)
            .and_then(Value::as_str)
            .map(String::from)
            .ok_or_else(|| format!("missing string `{key}`"))
    };
    if str_at(doc, "version")? != SARIF_VERSION {
        return Err("version must be 2.1.0".into());
    }
    str_at(doc, "$schema")?;
    let runs = doc
        .get("runs")
        .and_then(Value::as_array)
        .ok_or("missing `runs` array")?;
    for run in runs {
        let driver = run
            .get("tool")
            .and_then(|t| t.get("driver"))
            .ok_or("missing runs[].tool.driver")?;
        str_at(driver, "name")?;
        let results = run
            .get("results")
            .and_then(Value::as_array)
            .ok_or("missing runs[].results")?;
        for r in results {
            str_at(r, "ruleId")?;
            let locs = r
                .get("locations")
                .and_then(Value::as_array)
                .ok_or("result without locations")?;
            for loc in locs {
                let phys = loc
                    .get("physicalLocation")
                    .ok_or("location without physicalLocation")?;
                str_at(
                    phys.get("artifactLocation").ok_or("missing artifactLocation")?,
                    "uri",
                )?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> CiReport {
        CiReport {
            schema_version: CI_SCHEMA_VERSION,
            ok: false,
            project: "/p".into(),
            toolchain: ToolchainInfo {
                lean: None,
                lake: None,
            },
            build: BuildStatus {
                status: "failed",
                exit_code: Some(1),
                duration_ms: 12,
            },
            total_ms: 20,
            diagnostics: vec![Diagnostic {
                file: "src/Main.lean".into(),
                line: 3,
                column: 0,
                severity: Severity::Error,
                message: "unknown identifier".into(),
            }],
            failures: vec![FailureSummary {
                kind: "lean_error",
                file: Some("src/Main.lean".into()),
                line: Some(3),
                column: Some(0),
                message: "unknown identifier".into(),
            }],
            cache: CacheStats {
                available: false,
                hits: 0,
                misses: 0,
                entries: 0,
                note: "none",
            },
        }
    }

    #[test]
    fn sarif_has_required_keys() {
        let doc = to_sarif(&sample());
        assert_eq!(doc["version"], "2.1.0");
        assert!(doc["$schema"].as_str().unwrap().contains("sarif-schema-2.1.0"));
        assert_eq!(doc["runs"][0]["tool"]["driver"]["name"], "tpt-lean");
        assert_eq!(doc["runs"][0]["results"][0]["ruleId"], "lean/error");
        assert_eq!(doc["runs"][0]["results"][0]["level"], "error");
        let loc = &doc["runs"][0]["results"][0]["locations"][0]["physicalLocation"];
        assert_eq!(loc["artifactLocation"]["uri"], "src/Main.lean");
        assert_eq!(loc["region"]["startLine"], 3);
        assert_eq!(loc["region"]["startColumn"], 1);
        validate_sarif_shape(&doc).unwrap();
    }

    #[test]
    fn shape_check_rejects_missing_keys() {
        let mut doc = to_sarif(&sample());
        doc["runs"][0]["results"][0]
            .as_object_mut()
            .unwrap()
            .remove("ruleId");
        assert!(validate_sarif_shape(&doc).is_err());

        let mut doc = to_sarif(&sample());
        doc["version"] = json!("2.0.0");
        assert!(validate_sarif_shape(&doc).is_err());

        let mut doc = to_sarif(&sample());
        doc["runs"][0]["tool"]["driver"]
            .as_object_mut()
            .unwrap()
            .remove("name");
        assert!(validate_sarif_shape(&doc).is_err());
    }

    #[test]
    fn failure_without_location_becomes_notification() {
        let mut report = sample();
        report.diagnostics.clear();
        report.failures = vec![FailureSummary {
            kind: "build_failed",
            file: None,
            line: None,
            column: None,
            message: "lake build failed".into(),
        }];
        let doc = to_sarif(&report);
        assert_eq!(doc["runs"][0]["results"].as_array().unwrap().len(), 0);
        assert_eq!(
            doc["runs"][0]["invocations"][0]["toolExecutionNotifications"][0]["message"]["text"],
            "lake build failed"
        );
        assert_eq!(doc["runs"][0]["invocations"][0]["executionSuccessful"], false);
        validate_sarif_shape(&doc).unwrap();
    }

    #[test]
    fn missing_lake_reports_not_run() {
        let dir = std::env::temp_dir().join(format!("tpt-tools-ci-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tc = Toolchain {
            lean: None,
            lake: None,
        };
        let report = run_ci(&dir, &tc, &CiOptions::default()).unwrap();
        assert_eq!(report.build.status, "not_run");
        assert!(!report.ok);
        assert!(!report.cache.available);
        assert_eq!(report.cache.hits, 0);
        assert!(report.failures.iter().any(|f| f.kind == "environment"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
