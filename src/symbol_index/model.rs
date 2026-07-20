//! Public data types produced by the symbol index: symbol definitions, their
//! kind, and text-search references.

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
