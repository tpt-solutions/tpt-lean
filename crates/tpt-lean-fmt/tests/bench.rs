// SPDX-License-Identifier: MIT OR Apache-2.0
//! Timing check for the formatter against the target in todo.md (a 5,000-line file in under
//! 100 ms). Ignored by default because it is a measurement:
//! `cargo test -p tpt-lean-fmt --release --test bench -- --ignored --nocapture`.

use std::time::Instant;
use tpt_lean_fmt::format_text;

fn five_thousand_lines() -> String {
    let mut src = String::from("import Std\nimport Lake.Util\n\nnamespace Big\n");
    let mut n = 0;
    while n < 5000 {
        src.push_str(&format!(
            "/-- Block {n}. -/\ntheorem t{n} (a b : Nat) : a + b = b + a := by\n  intro x   \n  skip\n\n"
        ));
        n += 4;
    }
    src.push_str("end Big\n");
    src
}

#[test]
#[ignore = "measurement; run with --ignored --nocapture in release"]
fn format_five_thousand_lines_under_target() {
    let src = five_thousand_lines();
    let lines = src.lines().count();
    let _ = format_text(&src).unwrap(); // warm up
    let mut times = Vec::new();
    for _ in 0..10 {
        let t = Instant::now();
        let out = format_text(&src).unwrap();
        times.push(t.elapsed().as_secs_f64() * 1000.0);
        std::hint::black_box(out);
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = times[times.len() / 2];
    println!("formatted {lines} lines: median {median:.2} ms over {} runs", times.len());
    assert!(median < 100.0, "median {median:.2} ms exceeds the 100 ms target");
}
