// SPDX-License-Identifier: MIT OR Apache-2.0
//! Parsing of Lean/Lake diagnostic output into structured records.
//!
//! Lean prints diagnostics as `file:line:column: severity: message`, where the message may
//! continue on following lines. Columns are reported in Unicode code points by Lean's CLI.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub severity: Severity,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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

#[cfg(test)]
mod tests {
    use super::*;

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
