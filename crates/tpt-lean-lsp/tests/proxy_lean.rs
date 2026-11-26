// SPDX-License-Identifier: MIT OR Apache-2.0
//! End-to-end check of the proxy against the installed Lean server. Skips when `lean` or `lake`
//! is not installed.

use serde_json::{json, Value};
use std::io::{BufReader, Cursor};
use std::path::PathBuf;
use std::time::Duration;
use tpt_lean_adapter::{discover_toolchain, LspSession};
use tpt_lean_lsp::framing::{read_message, write_message};
use tpt_lean_lsp::Proxy;

fn frames(messages: &[Value]) -> Vec<u8> {
    let mut buf = Vec::new();
    for m in messages {
        write_message(&mut buf, m).unwrap();
    }
    buf
}

fn read_all(bytes: &[u8]) -> Vec<Value> {
    let mut reader = BufReader::new(Cursor::new(bytes.to_vec()));
    let mut out = Vec::new();
    while let Some(m) = read_message(&mut reader).unwrap() {
        out.push(m);
    }
    out
}

#[test]
fn hover_goal_and_diagnostics_through_proxy() {
    let tc = discover_toolchain();
    if tc.lean.is_none() || tc.lake.is_none() {
        eprintln!("skipping: lean/lake not installed");
        return;
    }
    let dir: PathBuf = std::env::temp_dir().join(format!("tpt-lsp-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("Proxy.lean");
    let text = "theorem t (a b : Nat) : a + b = b + a := by\n  skip\n";
    std::fs::write(&file, text).unwrap();
    let uri = tpt_lean_adapter::lsp::path_to_uri(&file);

    let session =
        LspSession::for_project(&dir, tc.lake.as_ref(), tc.lean.as_ref(), Duration::from_secs(120)).unwrap();
    let input = frames(&[
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
        json!({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {
            "textDocument": {"uri": uri, "languageId": "lean4", "version": 1, "text": text}}}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "textDocument/hover", "params": {
            "textDocument": {"uri": uri}, "position": {"line": 1, "character": 2}}}),
        json!({"jsonrpc": "2.0", "id": 3, "method": "shutdown"}),
        json!({"jsonrpc": "2.0", "method": "exit"}),
    ]);
    let mut proxy = Proxy::new(&dir, session, Vec::new());
    proxy.serve(BufReader::new(Cursor::new(input))).unwrap();
    let out = read_all(proxy.output());
    let _ = std::fs::remove_dir_all(&dir);

    let hover = out.iter().find(|m| m["id"] == 2).expect("hover response");
    let value = hover["result"]["contents"]["value"].as_str().unwrap_or_default();
    assert!(value.contains("a + b = b + a"), "hover: {hover}");

    let published = out
        .iter()
        .rfind(|m| m["method"] == "textDocument/publishDiagnostics" && m["params"]["uri"] == uri)
        .expect("diagnostics forwarded to the editor");
    assert!(
        !published["params"]["diagnostics"].as_array().unwrap().is_empty(),
        "expected the unsolved goal error: {published}"
    );
}

#[test]
fn code_actions_and_commands_through_proxy_with_lean() {
    use tpt_lean_core::dirs::ProjectDirs;

    let tc = discover_toolchain();
    if tc.lean.is_none() || tc.lake.is_none() {
        eprintln!("skipping: lean/lake not installed");
        return;
    }
    let dir: PathBuf = std::env::temp_dir().join(format!("tpt-lsp-actions-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".tpt-lean")).unwrap();
    std::fs::write(dir.join("Lib.lean"), "theorem widget_size_ok : True := trivial\n").unwrap();
    let mut index = tpt_lean_index::Index::open(&ProjectDirs::new(&dir).index_db()).unwrap();
    index
        .refresh(&tpt_lean_index::default_scan_roots(&dir, false))
        .unwrap();
    drop(index);
    let file = dir.join("Proxy.lean");
    let text = "theorem t (a b : Nat) : a + b = b + a := by\n  skip\n#check widget_size_ok\n";
    std::fs::write(&file, text).unwrap();
    let uri = tpt_lean_adapter::lsp::path_to_uri(&file);

    let session =
        LspSession::for_project(&dir, tc.lake.as_ref(), tc.lean.as_ref(), Duration::from_secs(120)).unwrap();
    let range = json!({"start": {"line": 2, "character": 7}, "end": {"line": 2, "character": 21}});
    let input = frames(&[
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
        json!({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {
            "textDocument": {"uri": uri, "languageId": "lean4", "version": 1, "text": text}}}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "textDocument/codeAction", "params": {
            "textDocument": {"uri": uri}, "range": range,
            "context": {"diagnostics": [{
                "range": range, "severity": 1,
                "message": "unknown identifier 'widget_size_ok'"}]}}}),
        json!({"jsonrpc": "2.0", "id": 3, "method": "workspace/executeCommand", "params": {
            "command": "tpt-lean.explainError",
            "arguments": [{"uri": uri, "line": 1, "character": 2}]}}),
        json!({"jsonrpc": "2.0", "id": 4, "method": "workspace/executeCommand", "params": {
            "command": "tpt-lean.searchLemmas",
            "arguments": [{"uri": uri, "line": 1, "character": 2}]}}),
        json!({"jsonrpc": "2.0", "id": 5, "method": "shutdown"}),
        json!({"jsonrpc": "2.0", "method": "exit"}),
    ]);
    let mut proxy = Proxy::new(&dir, session, Vec::new());
    proxy.serve(BufReader::new(Cursor::new(input))).unwrap();
    let out = read_all(proxy.output());
    let _ = std::fs::remove_dir_all(&dir);

    let actions = out.iter().find(|m| m["id"] == 2).expect("codeAction response")["result"]
        .as_array()
        .unwrap()
        .clone();
    assert!(actions.iter().any(|a| a["title"] == "Import Lib"), "{actions:?}");

    let explained = out.iter().find(|m| m["id"] == 3).expect("explain response");
    let goals = explained["result"]["goals"].as_array().expect("goals array");
    assert!(
        goals.iter().any(|g| g.as_str().unwrap_or("").contains("⊢")),
        "{explained}"
    );

    let lemmas = out.iter().find(|m| m["id"] == 4).expect("search response");
    assert!(lemmas["result"].is_array(), "{lemmas}");
}
