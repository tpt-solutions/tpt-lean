// SPDX-License-Identifier: MIT OR Apache-2.0
//! Timing of an incremental index refresh after one file changes, in a generated 500-file
//! project. A measurement, not a correctness test:
//! `cargo test -p tpt-lean-index --release --test bench -- --ignored --nocapture`.

use std::time::Instant;
use tpt_lean_index::{Index, ScanRoot};

#[test]
#[ignore = "measurement; run with --ignored --nocapture in release"]
fn incremental_refresh_after_one_edit() {
    let dir = std::env::temp_dir().join(format!("tpt-index-bench-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for f in 0..500 {
        let mut text = format!("namespace Bench.M{f}\n");
        for d in 0..10 {
            text.push_str(&format!(
                "/-- Lemma {d} in file {f}. -/\ntheorem lemma_{f}_{d} : True := trivial\n"
            ));
        }
        text.push_str(&format!("end Bench.M{f}\n"));
        std::fs::write(dir.join(format!("M{f}.lean")), text).unwrap();
    }
    let roots = [ScanRoot {
        package: "project".into(),
        path: dir.clone(),
    }];
    let db = dir.join("index.sqlite");
    let mut index = Index::open(&db).unwrap();

    let t = Instant::now();
    index.refresh(&roots).unwrap();
    let full = t.elapsed().as_secs_f64() * 1000.0;

    let mut times = Vec::new();
    for i in 0..10 {
        std::fs::write(
            dir.join("M7.lean"),
            format!("namespace Bench.M7\ntheorem edited_{i} : True := trivial\nend Bench.M7\n"),
        )
        .unwrap();
        let t = Instant::now();
        let stats = index.refresh(&roots).unwrap();
        times.push(t.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(stats.updated, 1, "only the edited file is re-read");
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!("full refresh of 500 files: {full:.0} ms");
    println!(
        "incremental refresh after one edit: p50 {:.0} ms, max {:.0} ms",
        times[times.len() / 2],
        times[times.len() - 1]
    );
    drop(index);
    let _ = std::fs::remove_dir_all(&dir);
}
