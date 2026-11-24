// SPDX-License-Identifier: MIT OR Apache-2.0
//! Controlled process execution: timeout, output limit, cancellation, and environment sanitization.
//!
//! The runner takes a program path and an argument vector. It never invokes a shell.
//! Timeouts, cancellation and output limits apply to the whole process tree: on expiry the
//! runner kills every descendant (via `taskkill /T` on Windows, a `pgrep` walk on Unix).
//!
//! Memory limits are enforced by a polling watchdog: when `ProcessPolicy::memory_limit_bytes`
//! is set, the summed resident memory of the process tree is sampled and the tree is killed
//! once it exceeds the limit. Sampling is periodic, so a process can briefly overshoot.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use sysinfo::{Pid, ProcessesToUpdate, System};
use tpt_lean_core::policy::{sanitize_env, CancelToken, ProcessPolicy};
use tpt_lean_core::{Error, ErrorCode, Result};

/// How often the memory watchdog samples the process tree.
const MEMORY_SAMPLE_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutput {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub cancelled: bool,
    pub output_truncated: bool,
    pub memory_exceeded: bool,
    pub duration: Duration,
}

impl ProcessOutput {
    pub fn success(&self) -> bool {
        self.exit_code == Some(0)
            && !self.timed_out
            && !self.cancelled
            && !self.output_truncated
            && !self.memory_exceeded
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
    for (k, v) in &policy.extra_env {
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
    let mut memory_exceeded = false;
    let mut watchdog = policy.memory_limit_bytes.map(|limit| (limit, System::new()));
    let mut last_sample: Option<Instant> = None;
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
        } else if let Some((limit, sys)) = watchdog.as_mut() {
            if last_sample.is_none_or(|t| t.elapsed() >= MEMORY_SAMPLE_INTERVAL) {
                last_sample = Some(Instant::now());
                if tree_memory(sys, child.id()) > *limit {
                    memory_exceeded = true;
                }
            }
            if !memory_exceeded {
                thread::sleep(Duration::from_millis(10));
                continue;
            }
        } else {
            thread::sleep(Duration::from_millis(10));
            continue;
        }
        kill_tree(child.id());
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
        memory_exceeded,
        duration: started.elapsed(),
    })
}

/// Total resident memory in bytes of `root` and all of its descendants.
fn tree_memory(sys: &mut System, root: u32) -> u64 {
    sys.refresh_processes(ProcessesToUpdate::All, true);
    let mut children: HashMap<Pid, Vec<Pid>> = HashMap::new();
    for (pid, proc_) in sys.processes() {
        if let Some(parent) = proc_.parent() {
            children.entry(parent).or_default().push(*pid);
        }
    }
    let mut total = 0u64;
    let mut stack = vec![Pid::from_u32(root)];
    while let Some(pid) = stack.pop() {
        if let Some(proc_) = sys.process(pid) {
            total = total.saturating_add(proc_.memory());
        }
        if let Some(kids) = children.get(&pid) {
            stack.extend(kids.iter().copied());
        }
    }
    total
}

/// Kill `pid` and every process it spawned. Called before killing the direct child, because
/// on Unix orphaned descendants are re-parented and can no longer be found from the child.
///
/// Uses fixed argument vectors only (`taskkill`, `pgrep`, `kill`); never a shell. Failures are
/// ignored: the direct child is still killed afterwards.
fn kill_tree(pid: u32) {
    if cfg!(windows) {
        let _ = std::process::Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        return;
    }
    // Unix: breadth-first walk of the parent/child relation, then KILL the whole set.
    let mut todo = vec![pid];
    let mut all = Vec::new();
    while let Some(parent) = todo.pop() {
        let Ok(out) = std::process::Command::new("pgrep")
            .args(["-P", &parent.to_string()])
            .stderr(Stdio::null())
            .output()
        else {
            break;
        };
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            if let Ok(child) = line.trim().parse::<u32>() {
                all.push(child);
                todo.push(child);
            }
        }
    }
    for victim in all {
        let _ = std::process::Command::new("kill")
            .args(["-KILL", &victim.to_string()])
            .stderr(Stdio::null())
            .status();
    }
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

    /// Whether a process with this pid is still running.
    fn pid_alive(pid: u32) -> bool {
        if cfg!(windows) {
            let out = std::process::Command::new("tasklist")
                .args(["/FI", &format!("PID eq {pid}"), "/NH"])
                .output()
                .expect("tasklist");
            String::from_utf8_lossy(&out.stdout).contains(&pid.to_string())
        } else {
            std::process::Command::new("kill")
                .args(["-0", &pid.to_string()])
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
    }

    /// A timed-out shell must not leave its grandchild running. The grandchild prints its pid
    /// first, so the test can check whether it survived `run`.
    #[test]
    fn timeout_kills_child_process_tree() {
        let (prog, args) = if cfg!(windows) {
            (
                PathBuf::from("cmd"),
                vec![
                    "/C".to_string(),
                    // Base64 of UTF-16LE `$PID; Start-Sleep -Seconds 60` (no quoting needed).
                    "powershell -NoProfile -EncodedCommand JABQAEkARAA7ACAAUwB0AGEAcgB0AC0AUwBsAGUAZQBwACAALQBTAGUAYwBvAG4AZABzACAANgAwAA==".to_string(),
                ],
            )
        } else {
            (
                PathBuf::from("/bin/sh"),
                vec![
                    "-c".to_string(),
                    "sh -c 'echo $$; exec sleep 60' & wait".to_string(),
                ],
            )
        };
        let policy = ProcessPolicy {
            timeout: Duration::from_secs(3),
            ..ProcessPolicy::default()
        };
        let started = Instant::now();
        let out = run(&prog, &args, None, &policy, &CancelToken::new()).unwrap();
        assert!(out.timed_out);
        // The grandchild sleeps 60 s and holds the output pipes; `run` must not wait for it.
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "run blocked on the grandchild for {:?}",
            started.elapsed()
        );
        let pid: u32 = out
            .stdout
            .lines()
            .find_map(|l| l.trim().parse().ok())
            .unwrap_or_else(|| panic!("no grandchild pid in output: {out:?}"));
        std::thread::sleep(Duration::from_millis(300));
        let alive = pid_alive(pid);
        if alive {
            // Don't leave the stray process behind for the rest of the test run.
            let _ = std::process::Command::new(if cfg!(windows) { "taskkill" } else { "kill" })
                .args(if cfg!(windows) {
                    vec!["/F".to_string(), "/PID".to_string(), pid.to_string()]
                } else {
                    vec!["-9".to_string(), pid.to_string()]
                })
                .status();
        }
        assert!(!alive, "grandchild pid {pid} survived the timeout");
    }

    fn sleep_program() -> (PathBuf, Vec<String>) {
        if cfg!(windows) {
            (
                PathBuf::from("cmd"),
                vec!["/C".to_string(), "ping -n 30 127.0.0.1 >NUL".to_string()],
            )
        } else {
            (
                PathBuf::from("/bin/sh"),
                vec!["-c".to_string(), "sleep 30".to_string()],
            )
        }
    }

    #[test]
    fn memory_limit_kills_process_tree() {
        let (prog, args) = sleep_program();
        let policy = ProcessPolicy {
            timeout: Duration::from_secs(20),
            memory_limit_bytes: Some(1), // any live process exceeds one byte
            ..ProcessPolicy::default()
        };
        let started = Instant::now();
        let out = run(&prog, &args, None, &policy, &CancelToken::new()).unwrap();
        assert!(out.memory_exceeded, "{out:?}");
        assert!(!out.timed_out);
        assert!(!out.success());
        assert!(started.elapsed() < Duration::from_secs(15), "{:?}", out.duration);
    }

    #[test]
    fn generous_memory_limit_does_not_trigger() {
        let policy = ProcessPolicy {
            memory_limit_bytes: Some(u64::MAX / 2),
            ..ProcessPolicy::default()
        };
        let out = run(
            Path::new("definitely-not-a-real-binary-xyz"),
            &[],
            None,
            &policy,
            &CancelToken::new(),
        );
        // Start failure is still reported as an error, not as a memory overrun.
        assert!(out.is_err());
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
        let out = run(&prog, &args, None, &policy, &CancelToken::new()).unwrap();
        assert!(out.success(), "{out:?}");
        assert!(!out.memory_exceeded);
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
