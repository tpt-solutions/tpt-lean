// SPDX-License-Identifier: MIT OR Apache-2.0
//! Integration tests against a real Lean/Lake install. They skip (pass with a notice)
//! when `lean` or `lake` is not on PATH or in ~/.elan/bin.

use std::path::PathBuf;
use tpt_lean_adapter::{
    discover_toolchain, execute, lean_policy, parse_version, Command, Severity, Toolchain,
};
use tpt_lean_core::policy::CancelToken;

fn toolchain() -> Option<Toolchain> {
    let tc = discover_toolchain();
    if tc.lean.is_some() && tc.lake.is_some() {
        Some(tc)
    } else {
        eprintln!("skipping: lean/lake not installed");
        None
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tpt-adapter-it-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn lean_version_parses() {
    let Some(tc) = toolchain() else { return };
    let r = execute(
        &tc,
        &Command::LeanVersion,
        None,
        &lean_policy(60),
        &CancelToken::new(),
    )
    .unwrap();
    assert!(r.output.success(), "{:?}", r.output);
    assert!(parse_version(&r.output.stdout).is_some());
}

#[test]
fn lean_check_reports_structured_error() {
    let Some(tc) = toolchain() else { return };
    let dir = temp_dir("check");
    let file = dir.join("Bad.lean");
    std::fs::write(&file, "example : 1 + 1 = 3 := by decide\n").unwrap();
    let r = execute(
        &tc,
        &Command::LeanCheck { file: file.clone() },
        Some(&dir),
        &lean_policy(120),
        &CancelToken::new(),
    )
    .unwrap();
    assert!(!r.output.success());
    let d = r
        .diagnostics
        .iter()
        .find(|d| d.severity == Severity::Error)
        .expect("an error diagnostic");
    assert_eq!(d.line, 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn lean_check_accepts_valid_file() {
    let Some(tc) = toolchain() else { return };
    let dir = temp_dir("ok");
    let file = dir.join("Good.lean");
    std::fs::write(
        &file,
        "theorem t (a b : Nat) : a + b = b + a := Nat.add_comm a b\n",
    )
    .unwrap();
    let r = execute(
        &tc,
        &Command::LeanCheck { file },
        Some(&dir),
        &lean_policy(120),
        &CancelToken::new(),
    )
    .unwrap();
    assert!(r.output.success(), "{:?}", r.output);
    assert!(r.diagnostics.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn plain_goal_from_language_server() {
    use std::time::Duration;
    use tpt_lean_adapter::lsp::{open_document, plain_goals, server_command, LspClient};
    let Some(tc) = toolchain() else { return };
    let dir = temp_dir("goal");
    let file = dir.join("Goal.lean");
    let text = "theorem t (a b : Nat) : a + b = b + a := by\n  skip\n";
    std::fs::write(&file, text).unwrap();
    let (program, args) = server_command(&dir, tc.lake.as_ref(), tc.lean.as_ref()).unwrap();
    let mut client = LspClient::start(&program, &args, &dir, Duration::from_secs(120)).unwrap();
    let uri = open_document(&mut client, &file, text).unwrap();
    let goals = plain_goals(&mut client, &uri, 1, 2, Duration::from_secs(120)).unwrap();
    client.shutdown(Duration::from_secs(10));
    assert!(
        goals.iter().any(|g| g.contains("⊢ a + b = b + a")),
        "unexpected goals: {goals:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

fn line_col(text: &str, needle: &str) -> (u32, u32) {
    let offset = text.find(needle).expect("needle in text");
    let before = &text[..offset];
    let line = before.matches('\n').count() as u32;
    let col = before.rsplit('\n').next().unwrap().chars().count() as u32;
    (line, col)
}

#[test]
fn plain_term_goal_from_language_server() {
    use std::time::Duration;
    use tpt_lean_adapter::lsp::{open_document, plain_term_goal, server_command, LspClient};
    let Some(tc) = toolchain() else { return };
    let dir = temp_dir("termgoal");
    let file = dir.join("Term.lean");
    let text = "theorem t (a b : Nat) : a + b = b + a := Nat.add_comm a b\n";
    std::fs::write(&file, text).unwrap();
    let (program, args) = server_command(&dir, tc.lake.as_ref(), tc.lean.as_ref()).unwrap();
    let mut client = LspClient::start(&program, &args, &dir, Duration::from_secs(120)).unwrap();
    let uri = open_document(&mut client, &file, text).unwrap();
    let (line, col) = line_col(text, "Nat.add_comm a b");
    let goal = plain_term_goal(&mut client, &uri, line, col, Duration::from_secs(120)).unwrap();
    client.shutdown(Duration::from_secs(10));
    let goal = goal.expect("a term goal at the proof term");
    assert!(goal.contains("a + b = b + a"), "unexpected term goal: {goal:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn did_change_reelaborates_document() {
    use std::time::Duration;
    use tpt_lean_adapter::lsp::{plain_goals, server_command, LspClient};
    let Some(tc) = toolchain() else { return };
    let dir = temp_dir("didchange");
    let file = dir.join("Change.lean");
    let first = "theorem t (a b : Nat) : a + b = b + a := by\n  skip\n";
    let second = "theorem t (a : Nat) : a + 0 = a := by\n  skip\n";
    std::fs::write(&file, first).unwrap();
    let (program, args) = server_command(&dir, tc.lake.as_ref(), tc.lean.as_ref()).unwrap();
    let mut client = LspClient::start(&program, &args, &dir, Duration::from_secs(120)).unwrap();
    let uri = client.sync_document(&file, first).unwrap();
    let before = plain_goals(&mut client, &uri, 1, 2, Duration::from_secs(120)).unwrap();
    assert!(before.iter().any(|g| g.contains("a + b = b + a")), "{before:?}");
    client.sync_document(&file, second).unwrap();
    let after = plain_goals(&mut client, &uri, 1, 2, Duration::from_secs(120)).unwrap();
    client.shutdown(Duration::from_secs(10));
    assert!(after.iter().any(|g| g.contains("a + 0 = a")), "{after:?}");
    assert!(!after.iter().any(|g| g.contains("a + b = b + a")), "{after:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cancelled_request_returns_promptly() {
    use std::time::{Duration, Instant};
    use tpt_lean_adapter::lsp::{is_cancelled, plain_goal_params, server_command, LspClient};
    use tpt_lean_core::policy::CancelToken;
    let Some(tc) = toolchain() else { return };
    let dir = temp_dir("cancel");
    let file = dir.join("Slow.lean");
    // The #eval blocks the server for a long time; the goal request waits behind it.
    let text = "#eval IO.sleep 30000\n\ntheorem t : True := by\n  trivial\n";
    std::fs::write(&file, text).unwrap();
    let (program, args) = server_command(&dir, tc.lake.as_ref(), tc.lean.as_ref()).unwrap();
    let mut client = LspClient::start(&program, &args, &dir, Duration::from_secs(120)).unwrap();
    let uri = client.sync_document(&file, text).unwrap();
    let (line, col) = line_col(text, "trivial");
    let cancel = CancelToken::new();
    let trigger = cancel.clone();
    let started = Instant::now();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        trigger.cancel();
    });
    let result = client.request_cancellable(
        "$/lean/plainGoal",
        plain_goal_params(&uri, line, col),
        Duration::from_secs(120),
        &cancel,
    );
    let elapsed = started.elapsed();
    client.shutdown(Duration::from_secs(10));
    let err = result.expect_err("the request must be cancelled");
    assert!(is_cancelled(&err), "{err:?}");
    assert!(elapsed < Duration::from_secs(5), "took {elapsed:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn session_restarts_after_server_is_killed() {
    use std::time::Duration;
    use tpt_lean_adapter::LspSession;
    let Some(tc) = toolchain() else { return };
    let dir = temp_dir("restart");
    let file = dir.join("Restart.lean");
    let text = "theorem t (a b : Nat) : a + b = b + a := by\n  skip\n";
    std::fs::write(&file, text).unwrap();
    let mut session =
        LspSession::for_project(&dir, tc.lake.as_ref(), tc.lean.as_ref(), Duration::from_secs(120)).unwrap();
    let uri = session.sync_document(&file, text).unwrap();
    let params = serde_json::json!({
        "textDocument": { "uri": uri },
        "position": { "line": 1, "character": 2 }
    });
    let goals = session.request("$/lean/plainGoal", params.clone()).unwrap();
    assert!(goals.to_string().contains("a + b = b + a"), "{goals}");
    assert!(
        session.server_version().is_some(),
        "initialize result carries serverInfo.version"
    );
    assert_eq!(session.restart_count(), 0);
    session.kill();
    let goals = session.request("$/lean/plainGoal", params).unwrap();
    assert!(goals.to_string().contains("a + b = b + a"), "{goals}");
    assert_eq!(session.restart_count(), 1);
    session.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
