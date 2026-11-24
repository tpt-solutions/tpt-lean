// SPDX-License-Identifier: MIT OR Apache-2.0
//! `tpt-lean` command-line interface.
//!
//! Global flags: `--json`, `--ndjson`, `--project <dir>`, `--set section.key=value`,
//! `--verbose`, `--quiet`. Exit codes follow `tpt_lean_core::ErrorCode::exit_code`.
//! Commands that are planned but not built yet fail with `not_implemented` (exit 10).

mod args;
mod commands;

use args::Args;
use tpt_lean_core::schema::{Envelope, ErrorEnvelope};
use tpt_lean_core::{Error, ErrorCode, Result};

/// Output mode selected by global flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Human,
    Json,
    Ndjson,
}

/// Shared output context passed to every command.
pub struct Ctx {
    pub mode: Mode,
    pub quiet: bool,
    pub project: Option<String>,
    pub overrides: Vec<(String, String)>,
    pub verbose: bool,
}

impl Ctx {
    /// Print a successful result. In JSON mode the human text is replaced by the envelope.
    pub fn emit<T: serde::Serialize>(&self, kind: &str, human: &str, data: &T) -> Result<()> {
        match self.mode {
            Mode::Human => {
                if !self.quiet && !human.is_empty() {
                    println!("{human}");
                }
            }
            Mode::Json | Mode::Ndjson => {
                let line = serde_json::to_string(&Envelope::ok(kind, data))
                    .map_err(|e| Error::new(ErrorCode::Internal, e.to_string()))?;
                println!("{line}");
            }
        }
        Ok(())
    }

    /// Print one record per line (NDJSON) or a human line. Used for list-shaped results.
    pub fn emit_items<T: serde::Serialize>(&self, kind: &str, human: &[String], items: &[T]) -> Result<()> {
        match self.mode {
            Mode::Human => {
                if !self.quiet {
                    for line in human {
                        println!("{line}");
                    }
                }
            }
            Mode::Ndjson => {
                for item in items {
                    let v = serde_json::json!({ "kind": kind, "item": item });
                    println!(
                        "{}",
                        serde_json::to_string(&v)
                            .map_err(|e| Error::new(ErrorCode::Internal, e.to_string()))?
                    );
                }
            }
            Mode::Json => {
                let body = serde_json::json!({ "items": items, "count": items.len() });
                println!(
                    "{}",
                    serde_json::to_string(&Envelope::ok(kind, body))
                        .map_err(|e| Error::new(ErrorCode::Internal, e.to_string()))?
                );
            }
        }
        Ok(())
    }

    /// Print a human-readable line to stderr (progress, notes). Silent in JSON mode.
    pub fn note(&self, msg: &str) {
        if self.mode == Mode::Human && !self.quiet {
            eprintln!("{msg}");
        }
    }
}

/// Entry point shared by both binaries. Returns the process exit code.
pub fn run_main() -> i32 {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = match Args::parse(&argv) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            return ErrorCode::InvalidArgument.exit_code();
        }
    };
    tpt_lean_core::init_logging(args.verbose, args.quiet);
    let ctx = Ctx {
        mode: if args.json {
            Mode::Json
        } else if args.ndjson {
            Mode::Ndjson
        } else {
            Mode::Human
        },
        quiet: args.quiet,
        project: args.project.clone(),
        overrides: args.overrides.clone(),
        verbose: args.verbose,
    };
    match commands::dispatch(&ctx, &args) {
        Ok(code) => code,
        Err(e) => report_error(&ctx, &e),
    }
}

fn report_error(ctx: &Ctx, e: &Error) -> i32 {
    match ctx.mode {
        Mode::Json | Mode::Ndjson => {
            let body = serde_json::to_string(&ErrorEnvelope::new(e)).unwrap_or_else(|_| "{}".into());
            println!("{body}");
        }
        Mode::Human => eprintln!("error: {e}"),
    }
    e.code().exit_code()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_envelope_has_stable_code() {
        let env = ErrorEnvelope::new(&Error::new(ErrorCode::Timeout, "slow"));
        let v = serde_json::to_value(&env).unwrap();
        assert_eq!(v["error"]["code"], "timeout");
        assert_eq!(v["ok"], false);
    }
}
