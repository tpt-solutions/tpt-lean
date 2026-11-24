// SPDX-License-Identifier: MIT OR Apache-2.0
//! Error types with stable, machine-readable codes.

use std::fmt;
use std::io;
use std::path::PathBuf;

/// Stable error codes. Codes are part of the public JSON contract: never renumber,
/// only append.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    InvalidArgument,
    Io,
    ConfigInvalid,
    ProjectNotFound,
    PathOutsideWorkspace,
    LeanNotFound,
    IndexError,
    SyntaxInvalid,
    Timeout,
    OutputLimitExceeded,
    NotImplemented,
    Internal,
}

impl ErrorCode {
    /// Stable string identifier used in JSON output.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::InvalidArgument => "invalid_argument",
            ErrorCode::Io => "io",
            ErrorCode::ConfigInvalid => "config_invalid",
            ErrorCode::ProjectNotFound => "project_not_found",
            ErrorCode::PathOutsideWorkspace => "path_outside_workspace",
            ErrorCode::LeanNotFound => "lean_not_found",
            ErrorCode::IndexError => "index_error",
            ErrorCode::SyntaxInvalid => "syntax_invalid",
            ErrorCode::Timeout => "timeout",
            ErrorCode::OutputLimitExceeded => "output_limit_exceeded",
            ErrorCode::NotImplemented => "not_implemented",
            ErrorCode::Internal => "internal",
        }
    }

    /// Process exit code for the CLI. Conventions are documented in `docs/cli.md`.
    pub fn exit_code(self) -> i32 {
        match self {
            ErrorCode::InvalidArgument => 2,
            ErrorCode::ProjectNotFound | ErrorCode::LeanNotFound => 3,
            ErrorCode::ConfigInvalid => 4,
            ErrorCode::PathOutsideWorkspace => 5,
            ErrorCode::IndexError => 6,
            ErrorCode::SyntaxInvalid => 7,
            ErrorCode::Timeout | ErrorCode::OutputLimitExceeded => 8,
            ErrorCode::Io => 9,
            ErrorCode::NotImplemented => 10,
            ErrorCode::Internal => 70,
        }
    }
}

/// The shared error type used across tpt-lean crates.
#[derive(Debug)]
pub struct Error {
    code: ErrorCode,
    message: String,
    path: Option<PathBuf>,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            path: None,
        }
    }

    pub fn with_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.path = Some(path.into());
        self
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidArgument, message)
    }

    pub fn not_implemented(what: impl Into<String>) -> Self {
        Self::new(
            ErrorCode::NotImplemented,
            format!("{} is not implemented yet", what.into()),
        )
    }

    pub fn code(&self) -> ErrorCode {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn path(&self) -> Option<&PathBuf> {
        self.path.as_ref()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.path {
            Some(p) => write!(f, "{}: {} ({})", self.code.as_str(), self.message, p.display()),
            None => write!(f, "{}: {}", self.code.as_str(), self.message),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::new(ErrorCode::Io, e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable_strings() {
        assert_eq!(ErrorCode::PathOutsideWorkspace.as_str(), "path_outside_workspace");
        assert_eq!(ErrorCode::Timeout.exit_code(), 8);
    }

    #[test]
    fn display_includes_code_and_path() {
        let e = Error::new(ErrorCode::Io, "boom").with_path("a/b");
        assert_eq!(e.to_string(), "io: boom (a/b)");
    }
}
