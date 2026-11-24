// SPDX-License-Identifier: MIT OR Apache-2.0
//! Security hardening tests: path safety fuzzing, symlink escape, device names, and timeouts.
//!
//! Symlink creation can need privileges on Windows. When it fails, that test prints a skip
//! message and returns without failing.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tpt_lean_core::paths::WorkspaceRoot;
use tpt_lean_core::policy::{CancelToken, ProcessPolicy};
use tpt_lean_core::ErrorCode;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("tpt-tools-sec-{tag}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// True when `candidate` is `root` or lies under it, compared the way WorkspaceRoot compares.
fn is_under(candidate: &Path, root: &Path) -> bool {
    if cfg!(windows) {
        let c = candidate.to_string_lossy().to_lowercase();
        let r = root.to_string_lossy().to_lowercase();
        let r = r.trim_end_matches(['\\', '/']).to_string();
        c == r || c.starts_with(&format!("{r}\\")) || c.starts_with(&format!("{r}/"))
    } else {
        candidate.starts_with(root)
    }
}

/// Deterministic pseudo-random generator (xorshift), so failures reproduce.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[(self.next_u64() % items.len() as u64) as usize]
    }
}

const FRAGMENTS: &[&str] = &[
    "..",
    ".",
    "/",
    "\\",
    "//",
    "\\\\",
    "C:",
    "c:\\",
    "/etc/passwd",
    "C:\\Windows\\System32",
    "\\\\?\\",
    "\\\\?\\C:\\",
    "\\\\server\\share",
    "file:",
    "%2e%2e",
    "%2F",
    "~",
    "src",
    "Main.lean",
    "CON",
    "nul",
    "AUX.lean",
    "COM1",
    "lpt9.txt",
    " ",
    "\t",
    "\u{202e}",
    "é",
    "..\\..",
    "./../",
    "link",
    "a:b",
    "*",
    "?",
    "\"",
    "<",
    ">",
    "|",
    "$HOME",
    "%USERPROFILE%",
    "\u{0}",
    "\u{feff}",
];

#[test]
fn fuzzed_paths_never_resolve_outside_root() {
    let tmp = TempDir::new("fuzz");
    let root = tmp.0.join("ws");
    fs::create_dir_all(root.join("src")).unwrap();
    let ws = WorkspaceRoot::new(&root).unwrap();

    let fixed = [
        "../../etc/passwd",
        "/etc/passwd",
        "C:\\Windows\\System32\\drivers\\etc\\hosts",
        "\\\\server\\share\\x",
        "src/../../outside.txt",
        "src/./../../../x",
        "NUL",
        "CON.lean",
        "src/COM3",
    ];
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut generated: Vec<String> = fixed.iter().map(|s| s.to_string()).collect();
    for _ in 0..3000 {
        let parts = 1 + (rng.next_u64() % 6) as usize;
        let mut s = String::new();
        for _ in 0..parts {
            let frag = rng.pick(FRAGMENTS);
            s.push_str(frag);
            if rng.next_u64() % 2 == 0 {
                s.push(if rng.next_u64() % 2 == 0 { '/' } else { '\\' });
            }
        }
        generated.push(s);
    }

    let mut accepted = 0usize;
    for input in &generated {
        match ws.resolve(input) {
            Ok(path) => {
                accepted += 1;
                assert!(
                    is_under(&path, ws.path()),
                    "accepted {input:?} resolved to {} outside {}",
                    path.display(),
                    ws.path().display()
                );
            }
            Err(e) => {
                assert!(
                    matches!(
                        e.code(),
                        ErrorCode::PathOutsideWorkspace | ErrorCode::InvalidArgument
                    ),
                    "unexpected error for {input:?}: {e}"
                );
            }
        }
    }
    // Sanity: the fuzzer does produce some accepted paths, so the assertion above is exercised.
    assert!(
        accepted > 0,
        "no input was accepted; the fuzz set is too hostile to be useful"
    );
}

#[test]
fn explicit_hostile_paths_are_rejected() {
    let tmp = TempDir::new("hostile");
    let root = tmp.0.join("ws");
    fs::create_dir_all(&root).unwrap();
    let ws = WorkspaceRoot::new(&root).unwrap();
    for input in ["../../etc/passwd", "..", "../ws2/x", "src/../../x"] {
        let err = ws.resolve(input).unwrap_err();
        assert_eq!(err.code(), ErrorCode::PathOutsideWorkspace, "{input}");
    }
    assert_eq!(ws.resolve("a\0b").unwrap_err().code(), ErrorCode::InvalidArgument);
}

/// Windows: a directory symlink if privileges allow, otherwise a junction (no privilege needed).
#[cfg(windows)]
fn make_dir_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    if let Ok(()) = std::os::windows::fs::symlink_dir(target, link) {
        return Ok(());
    }
    let status = std::process::Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(link)
        .arg(target)
        .stdout(std::process::Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other("mklink /J failed"))
    }
}

#[cfg(unix)]
fn make_dir_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[test]
fn symlink_inside_workspace_cannot_escape() {
    let tmp = TempDir::new("symlink");
    let root = tmp.0.join("ws");
    let outside = tmp.0.join("outside");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("secret.txt"), "classified").unwrap();
    let link = root.join("link");

    let created = make_dir_symlink(&outside, &link);
    if let Err(e) = created {
        eprintln!("skipping symlink escape test: cannot create symlink ({e})");
        return;
    }

    let ws = WorkspaceRoot::new(&root).unwrap();
    assert_eq!(
        ws.resolve("link/secret.txt").unwrap_err().code(),
        ErrorCode::PathOutsideWorkspace
    );
    assert_eq!(
        ws.resolve("link/new.lean").unwrap_err().code(),
        ErrorCode::PathOutsideWorkspace
    );
    assert_eq!(
        ws.resolve("link").unwrap_err().code(),
        ErrorCode::PathOutsideWorkspace
    );

    // The project file scanner must not descend into the link either.
    let files = tpt_lean_index::collect_lean_files(&root).unwrap();
    assert!(files.is_empty(), "scanner followed a symlink: {files:?}");
}

#[test]
fn device_file_names_are_rejected() {
    let tmp = TempDir::new("devices");
    let root = tmp.0.join("ws");
    fs::create_dir_all(root.join("src")).unwrap();
    let ws = WorkspaceRoot::new(&root).unwrap();
    for input in [
        "NUL",
        "nul",
        "CON",
        "src/CON",
        "AUX.lean",
        "src/PRN.txt",
        "COM1",
        "src/com9.log",
        "LPT1.x",
    ] {
        let err = ws.resolve(input).unwrap_err();
        assert_eq!(err.code(), ErrorCode::PathOutsideWorkspace, "{input}");
    }
    assert!(ws.resolve("src/CONTEXT.lean").is_ok());
}

#[test]
fn process_timeout_kills_sleeping_child() {
    // Run the sleeper directly, not through `cmd /C`: the runner kills only the direct child, so a
    // wrapper leaves its grandchild holding the output pipe open (observed: about 30 s wait).
    let (program, args) = if cfg!(windows) {
        (
            PathBuf::from("ping"),
            vec!["-n".to_string(), "30".to_string(), "127.0.0.1".to_string()],
        )
    } else {
        (
            PathBuf::from("/bin/sh"),
            vec!["-c".to_string(), "sleep 30".to_string()],
        )
    };
    let policy = ProcessPolicy {
        timeout: Duration::from_millis(500),
        ..ProcessPolicy::default()
    };
    let started = Instant::now();
    let out = tpt_lean_adapter::run(&program, &args, None, &policy, &CancelToken::new()).unwrap();
    assert!(out.timed_out, "{out:?}");
    assert!(!out.success());
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "child was not killed promptly: {:?}",
        started.elapsed()
    );
}
