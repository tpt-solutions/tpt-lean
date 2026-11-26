// SPDX-License-Identifier: MIT OR Apache-2.0
//! Timing check for project search through the proxy. The index is opened once per proxy, so
//! each `workspace/symbol` request is a query only. The timed test is ignored by default; run it
//! with `cargo test -p tpt-lean-lsp --test index_search -- --ignored --nocapture`.

use serde_json::{json, Value};
use std::io::{BufReader, Cursor};
use std::path::Path;
use std::time::{Duration, Instant};
use tpt_lean_adapter::LspSession;
use tpt_lean_core::dirs::ProjectDirs;
use tpt_lean_index::{default_scan_roots, Index};
use tpt_lean_lsp::framing::{read_message, write_message};
use tpt_lean_lsp::Proxy;

const FILES: usize = 40;
const DECLARATIONS_PER_FILE: usize = 100;
const REQUESTS: usize = 300;
/// Generous bound for one local query. A query over the index takes well under a millisecond.
const MAX_AVERAGE: Duration = Duration::from_millis(50);

fn write_project(dir: &Path) {
    for f in 0..FILES {
        let mut text = format!("namespace Bench{f}\n\n");
        for i in 0..DECLARATIONS_PER_FILE {
            text.push_str(&format!("theorem bench_{f}_{i}_lemma : True := trivial\n"));
        }
        text.push_str(&format!("\nend Bench{f}\n"));
        std::fs::write(dir.join(format!("Bench{f}.lean")), text).unwrap();
    }
}

#[test]
#[ignore = "timing benchmark; run with --ignored"]
fn instant_project_search() {
    let dir = std::env::temp_dir().join(format!("tpt-lsp-bench-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    write_project(&dir);
    let mut index = Index::open(&ProjectDirs::new(&dir).index_db()).unwrap();
    let started = Instant::now();
    index.refresh(&default_scan_roots(&dir, false)).unwrap();
    let build = started.elapsed();
    drop(index);

    let mut messages: Vec<Value> = (0..REQUESTS)
        .map(|i| {
            json!({"jsonrpc": "2.0", "id": i, "method": "workspace/symbol",
                   "params": {"query": format!("bench_{}_", i % FILES)}})
        })
        .collect();
    messages.push(json!({"jsonrpc": "2.0", "method": "exit"}));
    let mut bytes = Vec::new();
    for m in &messages {
        write_message(&mut bytes, m).unwrap();
    }

    // No server is ever started: workspace/symbol is answered from the index alone.
    let session = LspSession::new(
        Path::new("/nonexistent/tpt-lean-bench-server"),
        &[],
        &dir,
        Duration::from_secs(5),
    );
    let mut proxy = Proxy::new(&dir, session, Vec::new());
    let started = Instant::now();
    proxy.serve(BufReader::new(Cursor::new(bytes))).unwrap();
    let total = started.elapsed();
    let opens = proxy.index_opens();
    let average = total / REQUESTS as u32;

    let mut reader = BufReader::new(Cursor::new(proxy.output().clone()));
    let mut answered = 0;
    while let Some(m) = read_message(&mut reader).unwrap() {
        if m["result"].as_array().is_some_and(|a| !a.is_empty()) {
            answered += 1;
        }
    }
    drop(proxy);
    let _ = std::fs::remove_dir_all(&dir);

    println!(
        "index build {build:?} for {} declarations; {REQUESTS} workspace/symbol requests in {total:?} \
         (average {average:?}); index opened {opens} time(s); {answered} non-empty answers",
        FILES * DECLARATIONS_PER_FILE
    );
    assert_eq!(opens, 1);
    assert_eq!(answered, REQUESTS);
    assert!(
        average < MAX_AVERAGE,
        "average {average:?} exceeds {MAX_AVERAGE:?}"
    );
}
