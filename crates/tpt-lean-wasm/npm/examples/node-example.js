// SPDX-License-Identifier: MIT OR Apache-2.0
// Node example for tpt-lean-wasm. Requires the generated glue in ../pkg (see ../README.md).
"use strict";

const path = require("node:path");
const fs = require("node:fs");
const wasm = require(path.join(__dirname, "..", "pkg", "tpt_lean_wasm.js"));

const source = "import Mathlib\n\n/-- Addition is commutative. -/\ntheorem add_comm' (a b : Nat) : a + b = b + a := by\n  omega\n";

// 1. Tokenize
const tokens = JSON.parse(wasm.tokenize(source));
console.log("tokens:", tokens.tokens.length, "first:", tokens.tokens[0]);

// 2. Format (refuses when the parse is uncertain)
const formatted = JSON.parse(wasm.formatText("def  x :=  1   \n"));
console.log("format:", formatted.ok ? JSON.stringify(formatted.output) : formatted.error);

// 3. CST viewer
const cst = JSON.parse(wasm.cstTree(source));
console.log("cst root:", cst.tree.kind, "children:", cst.tree.children.length);

// 4. Search over a prebuilt index (JSON bytes). Build it with any tool that emits version 1.
const indexPath = process.argv[2];
const indexBytes = indexPath
  ? fs.readFileSync(indexPath)
  : Buffer.from(
      JSON.stringify({
        version: 1,
        entries: [
          {
            name: "Nat.add_comm",
            kind: "theorem",
            file: "Init/Nat.lean",
            line: 10,
            docstring: "Addition on naturals is commutative.",
          },
          { name: "Nat.add", kind: "def", file: "Init/Nat.lean", line: 2 },
        ],
      }),
    );

const index = new wasm.SearchIndex(new Uint8Array(indexBytes));
console.log("index entries:", index.len);
console.log("symbols 'nat.add':", index.searchSymbols("nat.add", 5));
console.log("docs 'commutative':", index.searchDocs("commutative", 5));
