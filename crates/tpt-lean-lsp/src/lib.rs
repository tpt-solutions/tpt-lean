// SPDX-License-Identifier: MIT OR Apache-2.0
//! tpt-lean LSP proxy (specification section 7.9). Editors talk to this server over stdio; it
//! forwards to Lean's own language server (`lake serve` or `lean --server`) and answers a few
//! requests itself: cached document symbols, workspace symbols from the index, and hover goals.
//!
//! Entry point: [`run_stdio`]. Protocol details are in `docs/lsp.md`.

pub mod framing;
pub mod proxy;
pub mod symbols;
pub mod workspace;

pub use proxy::{run_stdio, uri_to_path, Proxy};
