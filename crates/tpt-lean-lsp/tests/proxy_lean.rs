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
