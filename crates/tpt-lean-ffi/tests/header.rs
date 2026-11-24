// SPDX-License-Identifier: MIT OR Apache-2.0
//! The hand-written header is the ABI contract. Every `tptl_*` function it declares must be
//! exported from `src/lib.rs`, and every exported `tptl_*` function must be declared in the header.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// Collect every `tptl_<name>(` call-like token in `text`.
fn tptl_names(text: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let bytes = text.as_bytes();
    let mut search_from = 0;
    while let Some(pos) = text[search_from..].find("tptl_") {
        let start = search_from + pos;
        search_from = start + "tptl_".len();
        // Reject matches that are the tail of a longer identifier, such as `xtptl_`.
        if start > 0 && is_ident_byte(bytes[start - 1]) {
            continue;
        }
        let mut end = start;
        while end < bytes.len() && is_ident_byte(bytes[end]) {
            end += 1;
        }
        if bytes.get(end) == Some(&b'(') {
            names.insert(text[start..end].to_string());
        }
    }
    names
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

#[test]
fn header_declares_exactly_the_exported_functions() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let header = fs::read_to_string(root.join("include/tpt_lean.h")).expect("read header");
    let lib = fs::read_to_string(root.join("src/lib.rs")).expect("read src/lib.rs");

    // Exports are the `tptl_*` names on lines declaring an `extern "C"` function.
    let exported: BTreeSet<String> = lib
        .lines()
        .filter(|line| line.contains("extern \"C\" fn "))
        .flat_map(tptl_names)
        .collect();
    let declared = tptl_names(&header);

    assert!(
        !exported.is_empty(),
        "no exported tptl_* functions found in src/lib.rs"
    );
    let missing_from_header: Vec<_> = exported.difference(&declared).collect();
    let missing_from_lib: Vec<_> = declared.difference(&exported).collect();
    assert!(
        missing_from_header.is_empty(),
        "exported but not declared in include/tpt_lean.h: {missing_from_header:?}"
    );
    assert!(
        missing_from_lib.is_empty(),
        "declared in include/tpt_lean.h but not exported from src/lib.rs: {missing_from_lib:?}"
    );
}

#[test]
fn name_scanner_ignores_identifiers_that_only_contain_the_prefix() {
    let names = tptl_names("int xtptl_hidden(void); tptl_real(void); tptl_not_a_call;");
    assert_eq!(
        names.into_iter().collect::<Vec<_>>(),
        vec!["tptl_real".to_string()]
    );
}
