// SPDX-License-Identifier: MIT OR Apache-2.0
//! Shared foundation for tpt-lean: configuration, errors, project model, paths,
//! hashing and cache keys, process policy, JSON schema conventions, and logging.
//!
//! Lean remains the trusted proof checker. This crate contains no Lean semantics.

pub mod config;
pub mod dirs;
pub mod error;
pub mod hash;
pub mod paths;
pub mod policy;
pub mod project;
pub mod schema;

pub use config::{load_config, Config, ConfigLoader, ConfigSources};
pub use error::{Error, ErrorCode, Result};

/// Initialise logging to stderr. Honors `TPT_LEAN_LOG` (for example `debug` or `tpt_lean_core=trace`).
/// Safe to call more than once; later calls are ignored.
pub fn init_logging(verbose: bool, quiet: bool) {
    let default = if quiet {
        "error"
    } else if verbose {
        "debug"
    } else {
        "warn"
    };
    let filter = tracing_subscriber::EnvFilter::try_from_env("TPT_LEAN_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .without_time()
        .try_init();
}
