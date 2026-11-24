// SPDX-License-Identifier: MIT OR Apache-2.0
//! Controlled process execution: timeout, output limit, cancellation, and environment sanitization.
//!
//! The runner takes a program path and an argument vector. It never invokes a shell.
//! Memory limits are not enforced yet (`ProcessPolicy::memory_limit_bytes` is ignored); on
//! Windows that needs job objects, which this workspace does not implement yet.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tpt_lean_core::policy::{sanitize_env, CancelToken, ProcessPolicy};
use tpt_lean_core::{Error, ErrorCode, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutput {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub cancelled: bool,
    pub output_truncated: bool,
    pub duration: Duration,
}

impl ProcessOutput {
    pub fn success(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out && !self.cancelled && !self.output_truncated
    }
}

/// Output budget shared by stdout and stderr readers.
struct Budget {
    remaining: AtomicUsize,
    exceeded: AtomicBool,
}

/// Run `program` with `args` in `cwd` under `policy`. Returns captured output even when the
/// process fails; errors are reserved for failure to start it.
pub fn run(
    program: &Path,
    args: &[String],
    cwd: Option<&Path>,
    policy: &ProcessPolicy,
    cancel: &CancelToken,
) -> Result<ProcessOutput> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_clear();
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    for (k, v) in sanitize_env(std::env::vars(), &policy.env_allowlist) {
        cmd.env(k, v);
    }

    let started = Instant::now();
    let mut child = cmd.spawn().map_err(|e| {
        let code = if e.kind() == std::io::ErrorKind::NotFound {
            ErrorCode::LeanNotFound
        } else {
            ErrorCode::Io
        };
        Error::new(code, format!("failed to start {}: {e}", program.display()))
    })?;

    let budget = Arc::new(Budget {
        remaining: AtomicUsize::new(policy.output_limit_bytes),
        exceeded: AtomicBool::new(false),
    });
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");
    let (b1, b2) = (budget.clone(), budget.clone());
    let out_handle = thread::spawn(move || read_limited(stdout, &b1));
    let err_handle = thread::spawn(move || read_limited(stderr, &b2));

    let mut timed_out = false;
    let mut cancelled = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if cancel.is_cancelled() {
            cancelled = true;
        } else if started.elapsed() >= policy.timeout {
            timed_out = true;
        } else if budget.exceeded.load(Ordering::SeqCst) {
            // Output limit reached: stop the process rather than let it run unbounded.
        } else {
            thread::sleep(Duration::from_millis(10));
            continue;
        }
        let _ = child.kill();
        break child.wait().ok();
    };

    let (out_text, out_trunc) = out_handle.join().unwrap_or_else(|_| (String::new(), true));
    let (err_text, err_trunc) = err_handle.join().unwrap_or_else(|_| (String::new(), true));

    Ok(ProcessOutput {
        exit_code: status.and_then(|s| s.code()),
        stdout: out_text,
        stderr: err_text,
        timed_out,
        cancelled,
        output_truncated: out_trunc || err_trunc,
        duration: started.elapsed(),
    })
}

/// Read `reader` to EOF, keeping at most the remaining shared budget. Once the budget is gone,
/// keep draining (so the child never blocks on a full pipe) but store nothing more.
fn read_limited<R: Read>(mut reader: R, budget: &Budget) -> (String, bool) {
    let mut kept: Vec<u8> = Vec::new();
    let mut buf = [0u8; 8192];
    let mut truncated = false;
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let granted = take_budget(&budget.remaining, n);
        kept.extend_from_slice(&buf[..granted]);
        if granted < n {
            truncated = true;
            budget.exceeded.store(true, Ordering::SeqCst);
        }
    }
    (String::from_utf8_lossy(&kept).into_owned(), truncated)
}

/// Atomically take up to `want` bytes from the shared budget; returns how many were granted.
fn take_budget(remaining: &AtomicUsize, want: usize) -> usize {
    loop {
        let left = remaining.load(Ordering::SeqCst);
        let granted = left.min(want);
        if remaining
            .compare_exchange(left, left - granted, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return granted;
        }
    }
}

/// Default policy with the given timeout in seconds.
pub fn policy_with_timeout(seconds: u64) -> ProcessPolicy {
    ProcessPolicy {
        timeout: Duration::from_secs(seconds),
        ..ProcessPolicy::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn captures_stdout() {
        let (prog, args) = if cfg!(windows) {
            (
                PathBuf::from("cmd"),
                vec!["/C".to_string(), "echo hello".to_string()],
            )
        } else {
            (
                PathBuf::from("/bin/sh"),
                vec!["-c".to_string(), "echo hello".to_string()],
            )
        };
        let out = run(&prog, &args, None, &ProcessPolicy::default(), &CancelToken::new()).unwrap();
        assert!(out.success(), "{out:?}");
        assert!(out.stdout.to_lowercase().contains("hello"));
    }

    #[test]
    fn missing_program_is_reported() {
        let err = run(
            Path::new("definitely-not-a-real-binary-xyz"),
            &[],
            None,
            &ProcessPolicy::default(),
            &CancelToken::new(),
        )
        .unwrap_err();
        assert!(matches!(err.code(), ErrorCode::LeanNotFound | ErrorCode::Io));
    }

    #[test]
    fn timeout_kills_process() {
        let (prog, args) = if cfg!(windows) {
            (
                PathBuf::from("cmd"),
                vec!["/C".to_string(), "ping -n 20 127.0.0.1 >NUL".to_string()],
            )
        } else {
            (
                PathBuf::from("/bin/sh"),
                vec!["-c".to_string(), "sleep 20".to_string()],
            )
        };
        let policy = ProcessPolicy {
            timeout: Duration::from_millis(300),
            ..ProcessPolicy::default()
        };
        let out = run(&prog, &args, None, &policy, &CancelToken::new()).unwrap();
        assert!(out.timed_out);
        assert!(!out.success());
    }

    #[test]
    fn cancellation_stops_process() {
        let (prog, args) = if cfg!(windows) {
            (
                PathBuf::from("cmd"),
                vec!["/C".to_string(), "ping -n 20 127.0.0.1 >NUL".to_string()],
            )
        } else {
            (
                PathBuf::from("/bin/sh"),
                vec!["-c".to_string(), "sleep 20".to_string()],
            )
        };
        let cancel = CancelToken::new();
        let c2 = cancel.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            c2.cancel();
        });
        let out = run(&prog, &args, None, &ProcessPolicy::default(), &cancel).unwrap();
        assert!(out.cancelled);
        assert!(!out.success());
    }

    #[test]
    fn output_limit_is_enforced() {
        let (prog, args) = if cfg!(windows) {
            (
                PathBuf::from("cmd"),
                vec![
                    "/C".to_string(),
                    "for /L %i in (1,1,3000) do @echo 0123456789abcdef".to_string(),
                ],
            )
        } else {
            (
                PathBuf::from("/bin/sh"),
                vec![
                    "-c".to_string(),
                    "yes 0123456789abcdef | head -c 200000".to_string(),
                ],
            )
        };
        let policy = ProcessPolicy {
            output_limit_bytes: 1024,
            ..ProcessPolicy::default()
        };
        let out = run(&prog, &args, None, &policy, &CancelToken::new()).unwrap();
        assert!(out.output_truncated);
        assert!(out.stdout.len() <= 1024);
    }
}
