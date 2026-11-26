// SPDX-License-Identifier: MIT OR Apache-2.0
//! Parsing of Lean/Lake diagnostic output into structured records.
//!
//! Lean prints diagnostics as `file:line:column: severity: message`, where the message may
//! continue on following lines. Columns are reported in Unicode code points by Lean's CLI.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub severity: Severity,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// Parse Lean CLI output. Unrecognised leading lines are ignored.
pub fn parse_lean_output(text: &str) -> Vec<Diagnostic> {
    let mut out: Vec<Diagnostic> = Vec::new();
    for line in text.lines() {
        if let Some(d) = parse_header(line) {
            out.push(d);
        } else if let Some(last) = out.last_mut() {
            // Continuation of the previous message.
            last.message.push('\n');
            last.message.push_str(line);
        }
    }
    for d in &mut out {
        d.message = d.message.trim_end().to_string();
    }
    out
}

fn parse_header(line: &str) -> Option<Diagnostic> {
    parse_location_first(line).or_else(|| parse_severity_first(line))
}

/// `FILE:LINE:COLUMN: error: MESSAGE`, the form `lean` prints.
fn parse_severity_first(line: &str) -> Option<Diagnostic> {
    // Split from the right for the message so Windows drive letters (C:\...) are handled.
    let (head, message) = line
        .split_once(": error:")
        .map(|(h, m)| (h, (Severity::Error, m)))
        .or_else(|| {
            line.split_once(": warning:")
                .map(|(h, m)| (h, (Severity::Warning, m)))
        })
        .or_else(|| line.split_once(": info:").map(|(h, m)| (h, (Severity::Info, m))))?;
    let mut parts = head.rsplitn(3, ':');
    let column: u32 = parts.next()?.trim().parse().ok()?;
    let line_no: u32 = parts.next()?.trim().parse().ok()?;
    let file = parts.next()?.to_string();
    if file.is_empty() {
        return None;
    }
    Some(Diagnostic {
        file,
        line: line_no,
        column,
        severity: message.0,
        message: message.1.trim_start().to_string(),
    })
}

/// `error: FILE:LINE:COLUMN: MESSAGE`, the form Lake 5 prints for Lean's errors. The position is the
/// first `:LINE:COLUMN: ` after the file name, so a Windows drive letter (`C:\...`) is not mistaken
/// for it.
fn parse_location_first(line: &str) -> Option<Diagnostic> {
    let (severity, rest) = [
        ("error: ", Severity::Error),
        ("warning: ", Severity::Warning),
        ("info: ", Severity::Info),
    ]
    .iter()
    .find_map(|(prefix, sev)| line.strip_prefix(prefix).map(|rest| (*sev, rest)))?;
    let bytes = rest.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b != b':' {
            continue;
        }
        let Some(tail) = rest.get(i + 1..) else { continue };
        let digits_line: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits_line.is_empty() {
            continue;
        }
        let after_line = &tail[digits_line.len()..];
        let Some(after_colon) = after_line.strip_prefix(':') else { continue };
        let digits_col: String = after_colon.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits_col.is_empty() {
            continue;
        }
        let Some(message) = after_colon[digits_col.len()..].strip_prefix(": ") else { continue };
        let file = &rest[..i];
        if file.is_empty() {
            return None;
        }
        return Some(Diagnostic {
            file: file.to_string(),
            line: digits_line.parse().ok()?,
            column: digits_col.parse().ok()?,
            severity,
            message: message.trim_start().to_string(),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lake_style_location_first_errors() {
        let d = parse_lean_output(
            "error: Main.lean:1:26: Type mismatch
  rfl
has type
  ?m = ?m
",
        );
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].file, "Main.lean");
        assert_eq!((d[0].line, d[0].column), (1, 26));
        assert_eq!(d[0].severity, Severity::Error);
        assert_eq!(d[0].message, "Type mismatch
  rfl
has type
  ?m = ?m");
    }

    #[test]
    fn location_first_handles_windows_paths_and_colons_in_messages() {
        let d = parse_lean_output(
            "error: C:\\work\\Demo\\Main.lean:3:7: unknown identifier 'x:1:2: y'
",
        );
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].file, "C:\\work\\Demo\\Main.lean");
        assert_eq!((d[0].line, d[0].column), (3, 7));
        assert_eq!(d[0].message, "unknown identifier 'x:1:2: y'");
    }

    #[test]
    fn plain_lines_are_not_diagnostics() {
        assert!(parse_lean_output("Build completed successfully (5 jobs).
").is_empty());
        assert!(parse_lean_output("error: build failed
").is_empty());
    }

    #[test]
    fn parses_single_error() {
        let d = parse_lean_output("Main.lean:3:7: error: unknown identifier 'foo'\n");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].file, "Main.lean");
        assert_eq!((d[0].line, d[0].column), (3, 7));
        assert_eq!(d[0].severity, Severity::Error);
        assert_eq!(d[0].message, "unknown identifier 'foo'");
    }

    #[test]
    fn handles_windows_paths_and_continuations() {
        let text = "C:\\proj\\src\\Main.lean:10:2: warning: unused variable\n  `h`\nnote: x\nsrc/A.lean:1:1: error: boom\n";
        let d = parse_lean_output(text);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].file, "C:\\proj\\src\\Main.lean");
        assert_eq!(d[0].severity, Severity::Warning);
        assert!(d[0].message.contains("`h`"));
        assert_eq!(d[1].file, "src/A.lean");
    }

    #[test]
    fn ignores_noise() {
        assert!(parse_lean_output("building...\n").is_empty());
    }
}
