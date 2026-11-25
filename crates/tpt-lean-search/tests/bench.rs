// SPDX-License-Identifier: MIT OR Apache-2.0
//! Timing check for search against the performance targets in todo.md (warm p50 < 10 ms,
//! p95 < 50 ms). Ignored by default because it is a measurement, not a correctness test:
//! `cargo test -p tpt-lean-search --release --test bench -- --ignored --nocapture`.

use std::path::PathBuf;
use std::time::Instant;
use tpt_lean_index::{Index, ScanRoot};
use tpt_lean_search::{search, SearchOptions};

const FILES: usize = 500;
const DECLS_PER_FILE: usize = 10;

fn generated_project() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tpt-search-bench-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for f in 0..FILES {
        let mut text = format!("namespace Bench.M{f}\n");
        for d in 0..DECLS_PER_FILE {
            text.push_str(&format!(
                "/-- Lemma {d} about compact sets and continuous maps in file {f}. -/\ntheorem lemma_{f}_{d} (n m : Nat) : n + m = m + n := sorry\n"
            ));
        }
        text.push_str(&format!("end Bench.M{f}\n"));
        std::fs::write(dir.join(format!("M{f}.lean")), text).unwrap();
    }
    dir
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx]
}

#[test]
#[ignore = "measurement; run with --ignored --nocapture in release"]
fn search_latency_against_targets() {
    let dir = generated_project();
    let mut index = Index::open_in_memory().unwrap();
    let t = Instant::now();
    index
        .refresh(&[ScanRoot { package: "project".into(), path: dir.clone() }])
        .unwrap();
    let scan_ms = t.elapsed().as_secs_f64() * 1000.0;
    let stats = index.status().unwrap();
    println!("indexed {} symbols in {scan_ms:.0} ms", stats.symbols);

    let queries = [
        "lemma_250_5",          // exact full name
        "lemma_250",            // prefix
        "compact sets",         // docstring words (FTS)
        "continuous maps file", // docstring words, several terms
        "Bench.M42",            // namespace
        "nonexistent_thing",    // miss
    ];
    let opts = SearchOptions { limit: 10, ..Default::default() };
    for q in queries {
        let _ = search(&index, q, &opts).unwrap(); // warm up
    }
    let mut times = Vec::new();
    for _round in 0..50 {
        for q in queries {
            let t = Instant::now();
            let hits = search(&index, q, &opts).unwrap();
            times.push(t.elapsed().as_secs_f64() * 1000.0);
            std::hint::black_box(hits);
        }
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = percentile(&times, 0.50);
    let p95 = percentile(&times, 0.95);
    println!("search over {} queries: p50 {p50:.3} ms, p95 {p95:.3} ms", times.len());
    let _ = std::fs::remove_dir_all(&dir);
    assert!(p50 < 10.0, "p50 {p50:.3} ms exceeds the 10 ms target");
    assert!(p95 < 50.0, "p95 {p95:.3} ms exceeds the 50 ms target");
}
