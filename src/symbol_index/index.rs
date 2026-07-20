//! `SymbolIndex`: the thread-safe, tree-sitter-backed index itself — building
//! it by walking the repository, and the definition/implementation/reference
//! query methods used by code navigation.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Result;

use super::extract_go::extract_go_symbols;
use super::extract_rust::extract_rust_symbols;
use super::extract_ts::extract_ts_symbols;
use super::model::{Reference, Symbol, SymbolKind};

/// Inner data protected by a mutex.
pub(super) struct IndexData {
    // `pub(super)` so the `tests` sibling module can seed/inspect symbols
    // directly, mirroring the pre-split file where both lived in one module.
    pub(super) symbols: Vec<Symbol>,
    pub(super) available: bool,
}

/// Thread-safe tree-sitter-based symbol index.
pub struct SymbolIndex {
    root: Arc<Mutex<PathBuf>>,
    // `pub(super)` for the same reason as `IndexData`'s fields — the `tests`
    // module reaches directly into the mutex to seed fixture data.
    pub(super) data: Arc<Mutex<IndexData>>,
}

impl SymbolIndex {
    /// Create a new empty symbol index rooted at `root`.
    pub fn new(root: PathBuf) -> Self {
        Self {
            root: Arc::new(Mutex::new(root)),
            data: Arc::new(Mutex::new(IndexData {
                symbols: Vec::new(),
                available: false,
            })),
        }
    }

    /// Build the index by parsing Rust source files with tree-sitter.
    /// Returns the number of symbols indexed.
    pub fn build(&self) -> Result<usize> {
        let root = self.root.lock().unwrap().clone();

        let mut parser = tree_sitter::Parser::new();
        let mut symbols = Vec::new();

        let walker = ignore::WalkBuilder::new(&root)
            .hidden(true)
            .git_ignore(true)
            .build();

        for entry in walker.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

            let lang = match ext {
                "rs" => Lang::Rust,
                "go" => Lang::Go,
                "ts" | "tsx" => Lang::TypeScript,
                "js" | "jsx" => Lang::JavaScript,
                _ => continue,
            };

            let ts_lang: tree_sitter::Language = match lang {
                Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
                Lang::Go => tree_sitter_go::LANGUAGE.into(),
                Lang::TypeScript | Lang::JavaScript => {
                    if ext == "tsx" || ext == "jsx" {
                        tree_sitter_typescript::LANGUAGE_TSX.into()
                    } else {
                        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
                    }
                }
            };

            if parser.set_language(&ts_lang).is_err() {
                continue;
            }

            let source = match std::fs::read_to_string(path) {
                Ok(s) => s,
                Err(_) => continue,
            };

            let rel_path = path
                .strip_prefix(&root)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();

            let tree = match parser.parse(&source, None) {
                Some(t) => t,
                None => continue,
            };

            match lang {
                Lang::Rust => {
                    extract_rust_symbols(tree.root_node(), &source, &rel_path, &mut symbols)
                }
                Lang::Go => extract_go_symbols(tree.root_node(), &source, &rel_path, &mut symbols),
                Lang::TypeScript | Lang::JavaScript => {
                    extract_ts_symbols(tree.root_node(), &source, &rel_path, &mut symbols)
                }
            }
        }

        let count = symbols.len();
        let mut data = self.data.lock().unwrap();
        data.symbols = symbols;
        data.available = true;
        Ok(count)
    }

    /// Find definition symbols matching the given name.
    pub fn find_definitions(&self, name: &str) -> Vec<Symbol> {
        let data = self.data.lock().unwrap();
        data.symbols
            .iter()
            .filter(|s| {
                s.name == name && !matches!(s.kind, SymbolKind::Field | SymbolKind::EnumVariant)
            })
            .cloned()
            .collect()
    }

    /// Find implementation symbols matching the given name.
    pub fn find_implementations(&self, name: &str) -> Vec<Symbol> {
        let data = self.data.lock().unwrap();
        data.symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Impl && s.scope.as_deref() == Some(name))
            .cloned()
            .collect()
    }

    /// Find references to a symbol name by searching source files.
    /// Uses the `ignore` crate walker for respecting .gitignore.
    pub fn find_references(&self, name: &str, root: &Path) -> Vec<Reference> {
        let pattern = match regex::Regex::new(&format!(r"\b{}\b", regex::escape(name))) {
            Ok(p) => p,
            Err(_) => return Vec::new(),
        };

        let mut refs = Vec::new();
        let walker = ignore::WalkBuilder::new(root)
            .hidden(true)
            .git_ignore(true)
            .build();

        for entry in walker.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !matches!(
                ext,
                "rs" | "py"
                    | "js"
                    | "ts"
                    | "jsx"
                    | "tsx"
                    | "go"
                    | "c"
                    | "cpp"
                    | "h"
                    | "hpp"
                    | "java"
                    | "rb"
                    | "swift"
                    | "kt"
                    | "scala"
                    | "zig"
                    | "toml"
                    | "yaml"
                    | "yml"
                    | "json"
                    | "md"
            ) {
                continue;
            }

            let content = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let rel_path = path
                .strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();

            for (i, line) in content.lines().enumerate() {
                if pattern.is_match(line) {
                    refs.push(Reference {
                        file_path: rel_path.clone(),
                        line: i + 1,
                        content: line.to_string(),
                    });
                }
            }
        }

        refs
    }

    /// Whether the index has been built successfully.
    pub fn is_available(&self) -> bool {
        self.data.lock().unwrap().available
    }

    /// Return the root path of this index.
    pub fn root(&self) -> PathBuf {
        self.root.lock().unwrap().clone()
    }
}

// Allow cloning for background thread usage.
impl Clone for SymbolIndex {
    fn clone(&self) -> Self {
        Self {
            root: Arc::clone(&self.root),
            data: Arc::clone(&self.data),
        }
    }
}

// ── Language detection ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
enum Lang {
    Rust,
    Go,
    TypeScript,
    JavaScript,
}
