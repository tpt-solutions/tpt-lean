// SPDX-License-Identifier: MIT OR Apache-2.0
//! End-to-end checks against a real Lean toolchain. These need `lean` and `lake` on PATH or in
//! `~/.elan/bin`, and they take tens of seconds, so they are ignored by default:
//!
//! `cargo test -p tpt-lean-build --test real_lake -- --ignored --nocapture`
//!
//! The cache directory is redirected into a temporary directory for the duration of the test.

use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use tpt_lean_adapter::{discover_toolchain, Toolchain};
use tpt_lean_build::{build, watch, BuildEvent};
use tpt_lean_core::policy::CancelToken;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let p = std::env::temp_dir().join(format!("tpt-lean-build-e2e-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn lake_path(tc: &Toolchain) -> PathBuf {
    tc.lake.clone().expect("lake must be installed for this test")
}

fn names(events: &[BuildEvent]) -> Vec<String> {
    events
        .iter()
        .map(|e| match e {
            BuildEvent::FileStarted { file, .. }
            | BuildEvent::FileFinished { file, .. }
            | BuildEvent::FileFailed { file, .. } => format!("{}({file})", e.name()),
            _ => e.name().to_string(),
        })
        .collect()
}

fn print_events(label: &str, events: &[BuildEvent]) {
    println!("--- {label}");
    for e in events {
        println!("{}", serde_json::to_string(e).unwrap());
    }
}

#[test]
#[ignore = "needs a real Lean toolchain; run with --ignored"]
fn real_lake_build_and_watch() {
    let tmp = TempDir::new("project");
    let cache_home = tmp.0.join("cache-home");
    // Redirect the global cache into the temporary directory (single test in this binary).
    std::env::set_var("LOCALAPPDATA", &cache_home);
    std::env::set_var("XDG_CACHE_HOME", &cache_home);

    let tc = discover_toolchain();
    let proj = tmp.0.join("demo");
    let status = Command::new(lake_path(&tc))
        .args(["new", "demo"])
        .current_dir(&tmp.0)
        .status()
        .expect("lake new");
    assert!(status.success());

    // First build: cache miss, Lake builds everything.
    let mut first = Vec::new();
    let out1 = build(&proj, None, 300, &mut |e| first.push(e.clone())).expect("build 1");
    print_events("first build", &first);
    assert!(out1.success, "first build should succeed");
    assert!(!out1.cache_hit);
    assert!(first
        .iter()
        .any(|e| matches!(e, BuildEvent::FileFinished { file, .. } if file == "Demo/Basic.lean")));
    assert_eq!(first.first().map(|e| e.name()), Some("build_started"));
    assert_eq!(first.last().map(|e| e.name()), Some("build_finished"));
    assert!(names(&first).contains(&"cache_miss".to_string()));

    // Second build with no changes: key matches the recorded success, Lake builds nothing.
    let mut second = Vec::new();
    let out2 = build(&proj, None, 300, &mut |e| second.push(e.clone())).expect("build 2");
    print_events("unchanged rebuild", &second);
    assert!(out2.success);
    assert!(out2.cache_hit, "unchanged inputs should hit the disk record");
    assert_eq!(out2.files_built, 0);
    assert_eq!(out2.digest, out1.post_digest);
    println!(
        "key_ms(unchanged rebuild)={} duration_ms={}",
        out2.key_ms, out2.duration_ms
    );

    // Editing a source changes the key and makes Lake rebuild the module.
    let basic = proj.join("Demo").join("Basic.lean");
    let mut src = std::fs::read_to_string(&basic).unwrap();
    src.push_str("\ndef extra : Nat := 2\n");
    std::fs::write(&basic, src).unwrap();
    let mut third = Vec::new();
    let out3 = build(&proj, None, 300, &mut |e| third.push(e.clone())).expect("build 3");
    print_events("after editing Demo/Basic.lean", &third);
    assert!(out3.success);
    assert!(!out3.cache_hit);
    assert_ne!(out3.digest, out1.digest);
    assert!(out3.files_built >= 1);

    // Unchanged again after the edit: the new state is recorded, so this is a hit with no work.
    let mut fourth = Vec::new();
    let out4 = build(&proj, None, 300, &mut |e| fourth.push(e.clone())).expect("build 4");
    assert!(out4.cache_hit, "rebuild after an edit should hit the new record");
    assert_eq!(out4.files_built, 0);

    // Watch: edit a file once, wait for the rebuild, then cancel.
    let cancel = CancelToken::new();
    let mut watched: Vec<BuildEvent> = Vec::new();
    let watch_root = proj.clone();
    let c2 = cancel.clone();
    let handle = std::thread::spawn(move || {
        let mut local = Vec::new();
        let r = watch(&watch_root, None, 300, 200, &c2, &mut |e| local.push(e.clone()));
        (r, local)
    });
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let main_path = proj.join("Main.lean");
    let mut main_src = std::fs::read_to_string(&main_path).unwrap();
    main_src.push_str("\n-- touched\n");
    std::fs::write(&main_path, main_src).unwrap();
    // Allow poll + build (several seconds on a cold lake).
    std::thread::sleep(std::time::Duration::from_secs(15));
    cancel.cancel();
    let (r, local) = handle.join().expect("watch thread");
    watched.extend(local);
    r.expect("watch");
    print_events("watch after editing Main.lean", &watched);
    assert!(
        watched.iter().filter(|e| e.name() == "build_finished").count() >= 2,
        "the edit should have triggered a rebuild"
    );

    let explain = tpt_lean_build::explain_slow(&first, 3);
    println!("--- explain-slow (first build): {explain:?}");
}
