//! Tree-sitter-based symbol indexing for code navigation.
//!
//! Provides `SymbolIndex` which parses Rust source files using tree-sitter,
//! extracts symbol definitions, and supports definition/implementation/reference
//! lookups.
//!
//! Split by responsibility: [`model`] holds the public data types (`Symbol`,
//! `SymbolKind`, `Reference`), [`index`] holds the `SymbolIndex` type and its
//! build/query methods, `extract_common` holds the shared tree-sitter
//! AST-walking helpers, and `extract_rust`/`extract_go`/`extract_ts` hold the
//! per-language symbol extractors.

mod extract_common;
mod extract_go;
mod extract_rust;
mod extract_ts;
mod index;
mod model;
#[cfg(test)]
mod tests;

pub use index::SymbolIndex;
// `Reference` is consumed externally today (via `crate::symbol_index::Reference`
// in `app/`). `Symbol`/`SymbolKind` are currently only used within this module
// tree via `super::model::X`, but stay re-exported at the module root to
// preserve the pre-split `crate::symbol_index::X` path for any future caller.
#[allow(unused_imports)]
pub use model::{Reference, Symbol, SymbolKind};
