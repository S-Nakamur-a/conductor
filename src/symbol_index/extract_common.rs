//! Shared tree-sitter AST-walking helpers used by the per-language symbol
//! extractors (`extract_rust`, `extract_go`, `extract_ts`).

use super::model::{Symbol, SymbolKind};

/// Generic recursive AST walker that calls `visitor` for each node.
pub(super) fn walk_tree(
    node: tree_sitter::Node,
    source: &str,
    file_path: &str,
    symbols: &mut Vec<Symbol>,
    visitor: fn(tree_sitter::Node, &str, &str, &mut Vec<Symbol>),
) {
    visitor(node, source, file_path, symbols);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_tree(child, source, file_path, symbols, visitor);
    }
}

/// Extract a named symbol from a node that has a "name" field child.
pub(super) fn extract_named_symbol(
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
pub(super) fn node_text<'a>(node: tree_sitter::Node, source: &'a str) -> &'a str {
    &source[node.byte_range()]
}
