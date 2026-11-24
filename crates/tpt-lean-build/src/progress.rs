// SPDX-License-Identifier: MIT OR Apache-2.0
//! Parser for the progress lines `lake build` prints on stdout.
//!
//! Observed with Lake 5.0.0 / Lean 4.36.0 (Windows):
//! ```text
//! ✔ [2/11] Built Demo.Basic (542ms)
//! ✖ [2/11] Building Demo.Basic (505ms)     <- module whose build failed
//! Build completed successfully (11 jobs).
//! ```
//! Artifact jobs such as `Demo.Basic:c.o` and `demo:exe` are reported as [`Progress::Artifact`].
//! Unknown lines are ignored, so new Lake wording degrades to fewer events, not errors.

/// One recognised line of Lake progress output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    /// A module build started (only printed without a finish line, e.g. in failure output).
    Started { module: String },
    /// A module build finished successfully.
    Finished {
        module: String,
        duration_ms: Option<u64>,
    },
    /// A module build failed.
    Failed {
        module: String,
        duration_ms: Option<u64>,
    },
    /// A non-module job (object file, executable). Counted, but not reported as a file event.
    Artifact { name: String, failed: bool },
    /// The final summary line.
    Summary { success: bool },
}

/// Parse a duration such as `542ms` or `1.25s`. Only `ms` was observed in real output; `s` is
/// accepted defensively.
pub fn parse_duration_ms(text: &str) -> Option<u64> {
    let t = text.trim();
    if let Some(ms) = t.strip_suffix("ms") {
        return ms.trim().parse::<f64>().ok().map(|v| v.round() as u64);
    }
    if let Some(s) = t.strip_suffix('s') {
        return s.trim().parse::<f64>().ok().map(|v| (v * 1000.0).round() as u64);
    }
    None
}

/// Parse one line of Lake output. Returns `None` for lines that are not progress lines.
pub fn parse_line(line: &str) -> Option<Progress> {
    let trimmed = line.trim();
    if trimmed.starts_with("Build completed successfully") {
        return Some(Progress::Summary { success: true });
    }
    if trimmed.starts_with("Build failed") || trimmed == "error: build failed" {
        return Some(Progress::Summary { success: false });
    }
    let (failed_marker, rest) = match trimmed.strip_prefix('✖') {
        Some(r) => (true, r),
        None => (false, trimmed.strip_prefix('✔').unwrap_or(trimmed)),
    };
    let rest = rest.trim_start();
    // `[i/n] ` index prefix.
    if !rest.starts_with('[') {
        return None;
    }
    let close = rest.find(']')?;
    if !rest[1..close].contains('/') {
        return None;
    }
    let rest = rest[close + 1..].trim_start();
    let (building, body) = match rest.strip_prefix("Building ") {
        Some(b) => (true, b),
        None => (false, rest.strip_prefix("Built ")?),
    };
    // `name (duration)` tail; the duration is optional.
    let (name, duration_ms) = match body.rfind(" (") {
        Some(p) if body.ends_with(')') => {
            let d = parse_duration_ms(&body[p + 2..body.len() - 1]);
            (body[..p].trim(), d)
        }
        _ => (body.trim(), None),
    };
    if name.is_empty() {
        return None;
    }
    if name.contains(':') {
        return Some(Progress::Artifact {
            name: name.to_string(),
            failed: failed_marker,
        });
    }
    let module = name.to_string();
    Some(match (failed_marker, building) {
        (true, _) => Progress::Failed { module, duration_ms },
        (false, true) => Progress::Started { module },
        (false, false) => Progress::Finished { module, duration_ms },
    })
}

/// Parse all recognised progress lines from captured Lake output, in order.
pub fn parse_output(text: &str) -> Vec<Progress> {
    text.lines().filter_map(parse_line).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured from `lake build` on a fresh `lake new demo` project (Lean 4.36.0, Windows).
    const CAPTURED_SUCCESS: &str = "✔ [2/11] Built Demo.Basic (542ms)
✔ [4/11] Built Demo (536ms)
✔ [6/11] Built Demo.Basic:c.o (509ms)
✔ [7/11] Built Main (527ms)
✔ [9/11] Built Demo:c.o (496ms)
✔ [10/11] Built Main:c.o (83ms)
✔ [11/11] Built demo:exe (463ms)
Build completed successfully (11 jobs).
";

    // Captured after appending an ill-typed definition to Demo/Basic.lean (exit code 1).
    const CAPTURED_FAILURE: &str = "✖ [2/11] Building Demo.Basic (505ms)
trace: .> LEAN_PATH=x lean.exe Demo/Basic.lean --json
error: Demo/Basic.lean:4:17: Type mismatch
  \"x\"
has type
  String
but is expected to have type
  Nat
Some required targets logged failures:
- Demo.Basic
";

    #[test]
    fn parses_captured_success_output() {
        let p = parse_output(CAPTURED_SUCCESS);
        assert_eq!(p.len(), 8);
        assert_eq!(
            p[0],
            Progress::Finished {
                module: "Demo.Basic".into(),
                duration_ms: Some(542)
            }
        );
        assert_eq!(
            p[2],
            Progress::Artifact {
                name: "Demo.Basic:c.o".into(),
                failed: false
            }
        );
        assert_eq!(p[7], Progress::Summary { success: true });
    }

    #[test]
    fn parses_captured_failure_output() {
        let p = parse_output(CAPTURED_FAILURE);
        assert_eq!(
            p[0],
            Progress::Failed {
                module: "Demo.Basic".into(),
                duration_ms: Some(505)
            }
        );
        // Diagnostics and trace lines are not progress lines.
        assert_eq!(p.len(), 1);
    }

    #[test]
    fn ignores_unknown_lines() {
        assert!(parse_line("info: toolchain not updated; already up-to-date").is_none());
        assert!(parse_line("").is_none());
        assert!(parse_line("[x] Built Foo").is_none());
    }

    #[test]
    fn duration_forms() {
        assert_eq!(parse_duration_ms("542ms"), Some(542));
        assert_eq!(parse_duration_ms("1.25s"), Some(1250));
        assert_eq!(parse_duration_ms("soon"), None);
    }

    #[test]
    fn started_line_without_symbol() {
        assert_eq!(
            parse_line("[3/11] Building Demo"),
            Some(Progress::Started {
                module: "Demo".into()
            })
        );
    }
}
