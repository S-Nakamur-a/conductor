//! Tree-sitter-based symbol indexing for code navigation.
//!
//! Provides `SymbolIndex` which parses Rust source files using tree-sitter,
//! extracts symbol definitions, and supports definition/implementation/reference
//! lookups.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Result;

/// The kind of a symbol (function, struct, trait, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Function,
    Struct,
    Trait,
    Impl,
    Type,
    Const,
    Module,
    Enum,
    EnumVariant,
    Field,
    Method,
    Macro,
    Static,
    Interface,
    #[allow(dead_code)]
    Unknown,
}

/// A symbol definition found by tree-sitter parsing.
#[derive(Debug, Clone)]
pub struct Symbol {
    /// The symbol name (e.g. "MyStruct", "my_function").
    pub name: String,
    /// The kind of symbol.
    pub kind: SymbolKind,
    /// Relative file path from the repository root.
    pub file_path: String,
    /// 1-indexed line number.
    pub line: usize,
    /// 0-indexed column (if available).
    #[allow(dead_code)]
    pub column: usize,
    /// Scope (e.g. parent struct/module name), if available.
    pub scope: Option<String>,
}

/// A reference (usage) of a symbol found by text search.
#[derive(Debug, Clone)]
pub struct Reference {
    /// Relative file path from the repository root.
    pub file_path: String,
    /// 1-indexed line number.
    pub line: usize,
    /// The full text content of the line.
    pub content: String,
}

/// Inner data protected by a mutex.
struct IndexData {
    symbols: Vec<Symbol>,
    available: bool,
}

/// Thread-safe tree-sitter-based symbol index.
pub struct SymbolIndex {
    root: Arc<Mutex<PathBuf>>,
    data: Arc<Mutex<IndexData>>,
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
                Lang::Rust => extract_rust_symbols(tree.root_node(), &source, &rel_path, &mut symbols),
                Lang::Go => extract_go_symbols(tree.root_node(), &source, &rel_path, &mut symbols),
                Lang::TypeScript | Lang::JavaScript => extract_ts_symbols(tree.root_node(), &source, &rel_path, &mut symbols),
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
                s.name == name
                    && !matches!(s.kind, SymbolKind::Field | SymbolKind::EnumVariant)
            })
            .cloned()
            .collect()
    }

    /// Find implementation symbols matching the given name.
    pub fn find_implementations(&self, name: &str) -> Vec<Symbol> {
        let data = self.data.lock().unwrap();
        data.symbols
            .iter()
            .filter(|s| {
                s.kind == SymbolKind::Impl
                    && s.scope.as_deref() == Some(name)
            })
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

            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            if !matches!(
                ext,
                "rs" | "py" | "js" | "ts" | "jsx" | "tsx" | "go" | "c" | "cpp"
                    | "h" | "hpp" | "java" | "rb" | "swift" | "kt" | "scala"
                    | "zig" | "toml" | "yaml" | "yml" | "json" | "md"
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

// ── Tree-sitter AST walking ──────────────────────────────────────────

/// Generic recursive AST walker that calls `visitor` for each node.
fn walk_tree(node: tree_sitter::Node, source: &str, file_path: &str, symbols: &mut Vec<Symbol>,
    visitor: fn(tree_sitter::Node, &str, &str, &mut Vec<Symbol>),
) {
    visitor(node, source, file_path, symbols);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_tree(child, source, file_path, symbols, visitor);
    }
}

// ── Rust ──

fn extract_rust_symbols(root: tree_sitter::Node, source: &str, file_path: &str, symbols: &mut Vec<Symbol>) {
    walk_tree(root, source, file_path, symbols, visit_rust_node);
}

fn visit_rust_node(node: tree_sitter::Node, source: &str, file_path: &str, symbols: &mut Vec<Symbol>) {
    match node.kind() {
        "function_item" | "function_signature_item" => {
            if let Some(sym) = extract_named_symbol(node, source, file_path, SymbolKind::Function, "name") {
                symbols.push(sym);
            }
        }
        "struct_item" => {
            if let Some(sym) = extract_named_symbol(node, source, file_path, SymbolKind::Struct, "name") {
                symbols.push(sym);
            }
        }
        "enum_item" => {
            if let Some(sym) = extract_named_symbol(node, source, file_path, SymbolKind::Enum, "name") {
                symbols.push(sym);
            }
        }
        "trait_item" => {
            if let Some(sym) = extract_named_symbol(node, source, file_path, SymbolKind::Trait, "name") {
                symbols.push(sym);
            }
        }
        "impl_item" => {
            if let Some(type_node) = node.child_by_field_name("type") {
                let type_name = node_text(type_node, source).to_string();
                let line = node.start_position().row + 1;
                let column = node.start_position().column;
                symbols.push(Symbol {
                    name: format!("impl {type_name}"),
                    kind: SymbolKind::Impl,
                    file_path: file_path.to_string(),
                    line, column,
                    scope: Some(type_name),
                });
            }
        }
        "type_item" => {
            if let Some(sym) = extract_named_symbol(node, source, file_path, SymbolKind::Type, "name") {
                symbols.push(sym);
            }
        }
        "const_item" => {
            if let Some(sym) = extract_named_symbol(node, source, file_path, SymbolKind::Const, "name") {
                symbols.push(sym);
            }
        }
        "static_item" => {
            if let Some(sym) = extract_named_symbol(node, source, file_path, SymbolKind::Static, "name") {
                symbols.push(sym);
            }
        }
        "macro_definition" => {
            if let Some(sym) = extract_named_symbol(node, source, file_path, SymbolKind::Macro, "name") {
                symbols.push(sym);
            }
        }
        "mod_item" => {
            if let Some(sym) = extract_named_symbol(node, source, file_path, SymbolKind::Module, "name") {
                symbols.push(sym);
            }
        }
        "enum_variant" => {
            if let Some(sym) = extract_named_symbol(node, source, file_path, SymbolKind::EnumVariant, "name") {
                symbols.push(sym);
            }
        }
        "field_declaration" => {
            if let Some(sym) = extract_named_symbol(node, source, file_path, SymbolKind::Field, "name") {
                symbols.push(sym);
            }
        }
        _ => {}
    }
}

// ── Go ──

fn extract_go_symbols(root: tree_sitter::Node, source: &str, file_path: &str, symbols: &mut Vec<Symbol>) {
    walk_tree(root, source, file_path, symbols, visit_go_node);
}

fn visit_go_node(node: tree_sitter::Node, source: &str, file_path: &str, symbols: &mut Vec<Symbol>) {
    match node.kind() {
        "function_declaration" => {
            if let Some(sym) = extract_named_symbol(node, source, file_path, SymbolKind::Function, "name") {
                symbols.push(sym);
            }
        }
        "method_declaration" => {
            if let Some(sym) = extract_named_symbol(node, source, file_path, SymbolKind::Method, "name") {
                symbols.push(sym);
            }
        }
        "type_declaration" => {
            // type_declaration contains type_spec children.
        }
        "type_spec" => {
            if let Some(sym) = extract_named_symbol(node, source, file_path, SymbolKind::Type, "name") {
                // Check if it's a struct or interface.
                let kind = node.child_by_field_name("type")
                    .map(|t| match t.kind() {
                        "struct_type" => SymbolKind::Struct,
                        "interface_type" => SymbolKind::Interface,
                        _ => SymbolKind::Type,
                    })
                    .unwrap_or(SymbolKind::Type);
                symbols.push(Symbol { kind, ..sym });
            }
        }
        "const_spec" => {
            if let Some(sym) = extract_named_symbol(node, source, file_path, SymbolKind::Const, "name") {
                symbols.push(sym);
            }
        }
        "var_spec" => {
            if let Some(sym) = extract_named_symbol(node, source, file_path, SymbolKind::Static, "name") {
                symbols.push(sym);
            }
        }
        _ => {}
    }
}

// ── TypeScript / JavaScript ──

fn extract_ts_symbols(root: tree_sitter::Node, source: &str, file_path: &str, symbols: &mut Vec<Symbol>) {
    walk_tree(root, source, file_path, symbols, visit_ts_node);
}

fn visit_ts_node(node: tree_sitter::Node, source: &str, file_path: &str, symbols: &mut Vec<Symbol>) {
    match node.kind() {
        "function_declaration" => {
            if let Some(sym) = extract_named_symbol(node, source, file_path, SymbolKind::Function, "name") {
                symbols.push(sym);
            }
        }
        "class_declaration" => {
            if let Some(sym) = extract_named_symbol(node, source, file_path, SymbolKind::Struct, "name") {
                symbols.push(sym);
            }
        }
        "interface_declaration" => {
            if let Some(sym) = extract_named_symbol(node, source, file_path, SymbolKind::Interface, "name") {
                symbols.push(sym);
            }
        }
        "type_alias_declaration" => {
            if let Some(sym) = extract_named_symbol(node, source, file_path, SymbolKind::Type, "name") {
                symbols.push(sym);
            }
        }
        "enum_declaration" => {
            if let Some(sym) = extract_named_symbol(node, source, file_path, SymbolKind::Enum, "name") {
                symbols.push(sym);
            }
        }
        "method_definition" => {
            if let Some(sym) = extract_named_symbol(node, source, file_path, SymbolKind::Method, "name") {
                symbols.push(sym);
            }
        }
        "lexical_declaration" | "variable_declaration" => {
            // const Foo = ... / let bar = ... — extract variable declarators.
        }
        "variable_declarator" => {
            if let Some(sym) = extract_named_symbol(node, source, file_path, SymbolKind::Const, "name") {
                symbols.push(sym);
            }
        }
        "export_statement" => {
            // Recurse handled by walk_tree.
        }
        _ => {}
    }
}

/// Extract a named symbol from a node that has a "name" field child.
fn extract_named_symbol(
    node: tree_sitter::Node,
    source: &str,
    file_path: &str,
    kind: SymbolKind,
    name_field: &str,
) -> Option<Symbol> {
    let name_node = node.child_by_field_name(name_field)?;
    let name = node_text(name_node, source).to_string();
    if name.is_empty() {
        return None;
    }
    let line = name_node.start_position().row + 1;
    let column = name_node.start_position().column;
    Some(Symbol {
        name,
        kind,
        file_path: file_path.to_string(),
        line,
        column,
        scope: None,
    })
}

/// Get the text content of a tree-sitter node.
fn node_text<'a>(node: tree_sitter::Node, source: &'a str) -> &'a str {
    &source[node.byte_range()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_index_new() {
        let idx = SymbolIndex::new(PathBuf::from("/tmp"));
        assert!(!idx.is_available());
        assert_eq!(idx.root(), PathBuf::from("/tmp"));
    }

    #[test]
    fn test_find_definitions_empty() {
        let idx = SymbolIndex::new(PathBuf::from("/tmp"));
        let results = idx.find_definitions("foo");
        assert!(results.is_empty());
    }

    #[test]
    fn test_extract_symbols_from_rust_source() {
        let source = r#"
pub fn hello_world() {
    println!("hello");
}

struct MyStruct {
    field_a: u32,
}

enum Color {
    Red,
    Blue,
}

trait Drawable {
    fn draw(&self);
}

impl Drawable for MyStruct {
    fn draw(&self) {}
}

type Alias = Vec<u32>;

const MAX_SIZE: usize = 100;

static GLOBAL: &str = "test";

mod submodule;

macro_rules! my_macro {
    () => {};
}
"#;

        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(source, None).unwrap();

        let mut symbols = Vec::new();
        extract_rust_symbols(tree.root_node(), source, "test.rs", &mut symbols);

        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"hello_world"));
        assert!(names.contains(&"MyStruct"));
        assert!(names.contains(&"Color"));
        assert!(names.contains(&"Drawable"));
        assert!(names.contains(&"Alias"));
        assert!(names.contains(&"MAX_SIZE"));
        assert!(names.contains(&"GLOBAL"));
        assert!(names.contains(&"submodule"));
        assert!(names.contains(&"my_macro"));

        // Check enum variants.
        assert!(names.contains(&"Red"));
        assert!(names.contains(&"Blue"));

        // Check field.
        assert!(names.contains(&"field_a"));

        // Check impl — should have scope "MyStruct".
        let impl_sym = symbols.iter().find(|s| s.kind == SymbolKind::Impl).unwrap();
        assert_eq!(impl_sym.scope.as_deref(), Some("MyStruct"));

        // Check function inside impl.
        let draw_fns: Vec<_> = symbols.iter().filter(|s| s.name == "draw").collect();
        assert!(!draw_fns.is_empty());

        // Verify line numbers are 1-indexed and reasonable.
        let hello = symbols.iter().find(|s| s.name == "hello_world").unwrap();
        assert!(hello.line >= 1);
        assert_eq!(hello.kind, SymbolKind::Function);
    }

    #[test]
    fn test_find_definitions_filters_fields() {
        let idx = SymbolIndex::new(PathBuf::from("/tmp"));
        {
            let mut data = idx.data.lock().unwrap();
            data.symbols = vec![
                Symbol {
                    name: "Foo".to_string(),
                    kind: SymbolKind::Struct,
                    file_path: "lib.rs".to_string(),
                    line: 1,
                    column: 0,
                    scope: None,
                },
                Symbol {
                    name: "Foo".to_string(),
                    kind: SymbolKind::Field,
                    file_path: "lib.rs".to_string(),
                    line: 5,
                    column: 0,
                    scope: None,
                },
            ];
            data.available = true;
        }
        let defs = idx.find_definitions("Foo");
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].kind, SymbolKind::Struct);
    }

    #[test]
    fn test_find_implementations() {
        let idx = SymbolIndex::new(PathBuf::from("/tmp"));
        {
            let mut data = idx.data.lock().unwrap();
            data.symbols = vec![
                Symbol {
                    name: "impl MyStruct".to_string(),
                    kind: SymbolKind::Impl,
                    file_path: "lib.rs".to_string(),
                    line: 10,
                    column: 0,
                    scope: Some("MyStruct".to_string()),
                },
            ];
            data.available = true;
        }
        let impls = idx.find_implementations("MyStruct");
        assert_eq!(impls.len(), 1);
    }
}
