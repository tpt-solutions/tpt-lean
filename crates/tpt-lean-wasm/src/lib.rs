// SPDX-License-Identifier: MIT OR Apache-2.0
//! Restricted, browser-safe WebAssembly API for tpt-lean.
//!
//! Deliberately excluded: process spawning (Lean, Lake), native filesystem access, SQLite
//! indexes (`tpt-lean-index`), and any profiling or build orchestration. Everything here is
//! pure computation over strings and caller-supplied bytes.
//!
//! Exports (JavaScript names):
//! - `version()` -> string
//! - `tokenize(src)` -> JSON string, see [`text::tokenize_json`]
//! - `formatText(src)` -> JSON string, see [`text::format_json`]
//! - `cstTree(src)` -> JSON string, see [`text::cst_json`]
//! - `class SearchIndex`: `new SearchIndex(bytes)` (throws on invalid index), `.len`,
//!   `.searchSymbols(query, limit?)` and `.searchDocs(query, limit?)`, both returning JSON.
//!
//! Every JSON result carries an `ok` boolean. Offsets are UTF-8 byte offsets.

pub mod search;
pub mod text;

use wasm_bindgen::prelude::*;

/// Crate version string.
#[wasm_bindgen(js_name = version)]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Lossless token list for Lean source. Returns JSON `{"ok":true,"tokens":[...]}`.
#[wasm_bindgen(js_name = tokenize)]
pub fn tokenize(src: &str) -> String {
    text::tokenize_json(src)
}

/// Format Lean source. Returns JSON `{"ok":true,"output","changed"}`, or
/// `{"ok":false,"error","uncertain"}` when the formatter refuses.
#[wasm_bindgen(js_name = formatText)]
pub fn format_text(src: &str) -> String {
    text::format_json(src)
}

/// Shallow concrete syntax tree. Returns JSON `{"ok":true,"certain","tree"}`.
#[wasm_bindgen(js_name = cstTree)]
pub fn cst_tree(src: &str) -> String {
    text::cst_json(src)
}

/// A parsed, read-only search index. Create once and reuse across queries.
#[wasm_bindgen]
pub struct SearchIndex {
    inner: search::Index,
}

#[wasm_bindgen]
impl SearchIndex {
    /// Parse an index from JSON bytes (`Uint8Array`). Throws on malformed or unsupported input.
    #[wasm_bindgen(constructor)]
    pub fn new(bytes: &[u8]) -> Result<SearchIndex, JsError> {
        let inner = search::Index::from_json(bytes).map_err(|e| JsError::new(&e))?;
        Ok(SearchIndex { inner })
    }

    /// Number of entries in the index.
    #[wasm_bindgen(getter)]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// True when the index has no entries.
    #[wasm_bindgen(getter = isEmpty)]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Rank declaration names (exact, short-name exact, prefix, substring).
    /// Returns JSON `{"ok":true,"results":[{name,kind,file,line,score,docstring}]}`.
    #[wasm_bindgen(js_name = searchSymbols)]
    pub fn search_symbols(&self, query: &str, limit: Option<u32>) -> String {
        results_json(self.inner.search_symbols(query, limit.map(|l| l as usize)))
    }

    /// Rank docstrings (phrase match, all-terms match, plus name bonus).
    /// Returns JSON in the same shape as `searchSymbols`.
    #[wasm_bindgen(js_name = searchDocs)]
    pub fn search_docs(&self, query: &str, limit: Option<u32>) -> String {
        results_json(self.inner.search_docs(query, limit.map(|l| l as usize)))
    }
}

fn results_json(hits: Vec<search::SearchHit>) -> String {
    #[derive(serde::Serialize)]
    struct Out {
        ok: bool,
        results: Vec<search::SearchHit>,
    }
    serde_json::to_string(&Out {
        ok: true,
        results: hits,
    })
    .unwrap_or_else(|_| r#"{"ok":false,"results":[]}"#.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_matches_package() {
        assert_eq!(version(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn exports_return_ok_envelopes() {
        let t: serde_json::Value = serde_json::from_str(&tokenize("def x := 1")).unwrap();
        assert_eq!(t["ok"], true);
        let f: serde_json::Value = serde_json::from_str(&format_text("def x := 1\n")).unwrap();
        assert_eq!(f["ok"], true);
        let c: serde_json::Value = serde_json::from_str(&cst_tree("def x := 1\n")).unwrap();
        assert_eq!(c["ok"], true);
    }

    #[test]
    fn results_envelope_shape() {
        let idx = search::Index::from_json(
            br#"{"version":1,"entries":[{"name":"Nat.zero","kind":"def","file":"f.lean","line":4}]}"#,
        )
        .unwrap();
        let out: serde_json::Value =
            serde_json::from_str(&results_json(idx.search_symbols("zero", None))).unwrap();
        assert_eq!(out["ok"], true);
        assert_eq!(out["results"][0]["name"], "Nat.zero");
        assert_eq!(out["results"][0]["line"], 4);
        assert!(out["results"][0]["docstring"].is_null());
    }
}
