// SPDX-License-Identifier: MIT OR Apache-2.0
//! wasm-bindgen tests. Run on Node with:
//! `CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner cargo test -p tpt-lean-wasm --target wasm32-unknown-unknown`

#![cfg(target_arch = "wasm32")]

use tpt_lean_wasm::{cst_tree, format_text, tokenize, SearchIndex};
use wasm_bindgen_test::*;

const INDEX: &str = r#"{"version":1,"entries":[
    {"name":"Nat.add_comm","kind":"theorem","file":"Init/Nat.lean","line":10,"docstring":"Addition is commutative."},
    {"name":"Nat.add","kind":"def","file":"Init/Nat.lean","line":2}
]}"#;

#[wasm_bindgen_test]
fn tokenize_runs_in_wasm() {
    let out = tokenize("theorem t : True := trivial");
    assert!(out.contains(r#""kind":"keyword""#));
    assert!(out.contains(r#""ok":true"#));
}

#[wasm_bindgen_test]
fn format_runs_in_wasm() {
    let out = format_text("def  x := 1   \n");
    assert!(out.contains(r#""ok":true"#));
    assert!(out.contains(r#""changed":true"#));
}

#[wasm_bindgen_test]
fn cst_runs_in_wasm() {
    let out = cst_tree("def x := 1\n");
    assert!(out.contains(r#""kind":"decl""#));
}

#[wasm_bindgen_test]
fn search_index_from_bytes() {
    let idx = SearchIndex::new(INDEX.as_bytes()).expect("valid index");
    assert_eq!(idx.len(), 2);
    let hits = idx.search_symbols("nat.add", Some(5));
    assert!(hits.contains(r#""name":"Nat.add""#));
    let docs = idx.search_docs("commutative", None);
    assert!(docs.contains(r#""name":"Nat.add_comm""#));
}

#[wasm_bindgen_test]
fn search_index_rejects_bad_input() {
    assert!(SearchIndex::new(b"{").is_err());
}
