// SPDX-License-Identifier: MIT OR Apache-2.0
//! Golden tests: each directory under `tests/golden/` holds `input.lean` and
//! `expected_formatted.lean`. Formatting must produce the expected text, and be idempotent.

use std::fs;
use std::path::Path;

#[test]
fn golden_cases_format_as_expected() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("golden");
    let mut ran = 0;
    for entry in fs::read_dir(&root).expect("golden dir") {
        let dir = entry.unwrap().path();
        if !dir.is_dir() {
            continue;
        }
        let input = fs::read_to_string(dir.join("input.lean")).expect("input.lean");
        let expected =
            fs::read_to_string(dir.join("expected_formatted.lean")).expect("expected_formatted.lean");
        let once = tpt_lean_fmt::format_text(&input)
            .expect("golden input must format")
            .output;
        assert_eq!(once, expected, "golden mismatch in {}", dir.display());
        let twice = tpt_lean_fmt::format_text(&once)
            .expect("formatted output must format")
            .output;
        assert_eq!(twice, once, "not idempotent in {}", dir.display());
        ran += 1;
    }
    assert!(ran >= 1, "no golden cases found");
}
