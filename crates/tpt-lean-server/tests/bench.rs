// SPDX-License-Identifier: MIT OR Apache-2.0
//! Latency of `context.pack` through a warm Lean session, against the target in todo.md (p50 under
//! 100 ms, p95 under 500 ms, excluding elaboration). Ignored by default because it needs Lean and is
//! a measurement: `cargo test -p tpt-lean-server --release --test bench -- --ignored --nocapture`.

use std::path::PathBuf;
use std::time::Instant;

use serde_json::json;
use tpt_lean_server::Server;

fn project() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tpt-server-bench-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("Ctx.lean"),
        "theorem t (a b : Nat) : a + b = b + a := by\n  skip\n",
    )
    .unwrap();
    dir
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    sorted[((sorted.len() as f64 - 1.0) * p).round() as usize]
}

fn call(server: &Server) -> f64 {
    let t = Instant::now();
    let reply = server.handle(&json!({
        "jsonrpc": "2.0", "id": 1, "method": "context.pack",
        "params": { "path": "Ctx.lean", "line": 2, "column": 3 }
    }));
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    assert!(reply.is_some_and(|r| r.get("result").is_some()));
    ms
}

#[test]
#[ignore = "measurement; needs Lean; run with --ignored --nocapture"]
fn warm_context_pack_latency() {
    if tpt_lean_adapter::discover_toolchain().lean.is_none() {
        eprintln!("skipping: lean not found");
        return;
    }
    let dir = project();
    let server = Server::new(&dir).unwrap();
    let cold = call(&server);
    println!(
        "first call (starts Lean): {cold:.0} ms, starts {}",
        server.warm_session_starts()
    );

    let mut warm = Vec::new();
    for _ in 0..30 {
        warm.push(call(&server));
    }
    warm.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!(
        "warm, same text, 30 calls: p50 {:.0} ms, p95 {:.0} ms",
        percentile(&warm, 0.5),
        percentile(&warm, 0.95)
    );

    // Edits: each change resends the document and waits for Lean to elaborate it.
    let mut edited = Vec::new();
    for i in 0..10 {
        std::fs::write(
            dir.join("Ctx.lean"),
            format!("theorem t (a b : Nat) : a + b = b + a := by\n  skip\n-- edit {i}\n"),
        )
        .unwrap();
        edited.push(call(&server));
    }
    edited.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!(
        "warm, edited text, 10 calls: p50 {:.0} ms, p95 {:.0} ms",
        percentile(&edited, 0.5),
        percentile(&edited, 0.95)
    );
    server.release_warm_session();
    let _ = std::fs::remove_dir_all(&dir);
}
