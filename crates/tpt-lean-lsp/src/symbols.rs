// SPDX-License-Identifier: MIT OR Apache-2.0
//! Document symbols computed from the lossless syntax layer, so they are available before Lean
//! has finished elaborating the file.

use serde_json::{json, Value};
use tpt_lean_syntax::position::LineIndex;
use tpt_lean_syntax::{parse, Symbol};

/// LSP `SymbolKind` for a Lean declaration keyword.
pub fn symbol_kind(decl_kind: &str) -> u32 {
    match decl_kind {
        "structure" => 23,        // Struct
        "class" => 5,             // Class
        "inductive" => 10,        // Enum
        "axiom" | "opaque" => 14, // Constant
        "instance" => 6,          // Method
        _ => 12,                  // Function (def, theorem, lemma, abbrev, ...)
    }
}

/// Flat `DocumentSymbol` list for `text`. Ranges are the declarations' source spans.
pub fn document_symbols(text: &str) -> Vec<Value> {
    let parsed = parse(text);
    parsed
        .symbols
        .iter()
        .map(|s| document_symbol(s, &parsed.lines))
        .collect()
}

fn document_symbol(symbol: &Symbol, lines: &LineIndex) -> Value {
    let start = lines.position(symbol.range.start);
    let end = lines.position(symbol.range.end);
    let range = json!({
        "start": { "line": start.line.saturating_sub(1), "character": start.column_utf16 },
        "end": { "line": end.line.saturating_sub(1), "character": end.column_utf16 }
    });
    let name = symbol.short_name.clone().unwrap_or_else(|| symbol.name.clone());
    json!({
        "name": name,
        "detail": symbol.kind,
        "kind": symbol_kind(&symbol.kind),
        "range": range,
        "selectionRange": range,
        "children": []
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declarations_become_symbols() {
        let text =
            "namespace Foo\n\ntheorem bar : True := trivial\n\nstructure Baz where\n  x : Nat\n\nend Foo\n";
        let symbols = document_symbols(text);
        let names: Vec<&str> = symbols.iter().filter_map(|s| s["name"].as_str()).collect();
        assert!(names.iter().any(|n| n.ends_with("bar")), "{names:?}");
        assert!(names.iter().any(|n| n.ends_with("Baz")), "{names:?}");
        let baz = symbols
            .iter()
            .find(|s| s["name"].as_str().is_some_and(|n| n.ends_with("Baz")))
            .unwrap();
        assert_eq!(baz["kind"], 23);
        assert_eq!(baz["range"]["start"]["line"], 4);
    }

    #[test]
    fn kinds_map_sensibly() {
        assert_eq!(symbol_kind("structure"), 23);
        assert_eq!(symbol_kind("theorem"), 12);
    }
}
