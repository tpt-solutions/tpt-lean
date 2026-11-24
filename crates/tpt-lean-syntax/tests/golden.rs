// SPDX-License-Identifier: MIT OR Apache-2.0
//! Golden tests: each directory under `tests/golden/` holds an `input.lean` and the expected
//! symbol listing in `expected_symbols.txt`.

use std::fs;
use std::path::Path;

fn render(src: &str) -> String {
    let p = tpt_lean_syntax::parse(src);
    let mods: Vec<&str> = p.imports.iter().map(|i| i.module.as_str()).collect();
    let mut out = format!("imports {}\n", mods.join(","));
    for s in &p.symbols {
        out.push_str(&format!(
            "line={} kind={} name={} private={} doc={}\n",
            s.line,
            s.kind,
            s.name,
            s.is_private,
            s.docstring.as_deref().unwrap_or("-")
        ));
    }
    out
}

#[test]
fn golden_cases_match() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("golden");
    let mut ran = 0;
    for entry in fs::read_dir(&root).expect("golden dir") {
        let dir = entry.unwrap().path();
        if !dir.is_dir() {
            continue;
        }
        let src = fs::read_to_string(dir.join("input.lean")).expect("input.lean");
        let expected = fs::read_to_string(dir.join("expected_symbols.txt")).expect("expected_symbols.txt");
        assert_eq!(render(&src), expected, "golden mismatch in {}", dir.display());
        // Round-trip must be exact for golden inputs too.
        assert_eq!(tpt_lean_syntax::parse(&src).to_text(&src), src);
        ran += 1;
    }
    assert!(ran >= 1, "no golden cases found");
}
