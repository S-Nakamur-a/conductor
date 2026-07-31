//! Shared tree-sitter AST-walking helpers used by the per-language symbol
//! extractors (`extract_rust`, `extract_go`, `extract_ts`).

use super::model::{Symbol, SymbolKind};

/// Generic AST walker that calls `visitor` for each node, in pre-order.
///
/// Iterative rather than recursive so that one `TreeCursor` is threaded
/// through the whole traversal. The recursive form called `node.walk()` at
/// every node, and each of those allocates — on this repository that was tens
/// of thousands of short-lived cursors and about 30% of the extraction pass.
pub(super) fn walk_tree(
    node: tree_sitter::Node,
    source: &str,
    file_path: &str,
    symbols: &mut Vec<Symbol>,
    visitor: fn(tree_sitter::Node, &str, &str, &mut Vec<Symbol>),
) {
    let mut cursor = node.walk();
    loop {
        visitor(cursor.node(), source, file_path, symbols);
        if cursor.goto_first_child() {
            continue;
        }
        // No children: climb until a sibling appears, or until we run out of
        // parents, which means the subtree rooted at `node` is exhausted.
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return;
            }
        }
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
