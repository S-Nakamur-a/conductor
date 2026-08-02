//! Go のシンボル抽出: 関数、メソッド、型宣言（構造体・インターフェース）、
//! 定数、変数。

use super::extract_common::{extract_named_symbol, walk_tree};
use super::model::{Symbol, SymbolKind};

pub(super) fn extract_go_symbols(
    root: tree_sitter::Node,
    source: &str,
    file_path: &str,
    symbols: &mut Vec<Symbol>,
) {
    walk_tree(root, source, file_path, symbols, visit_go_node);
}

fn visit_go_node(
    node: tree_sitter::Node,
    source: &str,
    file_path: &str,
    symbols: &mut Vec<Symbol>,
) {
    match node.kind() {
        "function_declaration" => {
            if let Some(sym) =
                extract_named_symbol(node, source, file_path, SymbolKind::Function, "name")
            {
                symbols.push(sym);
            }
        }
        "method_declaration" => {
            if let Some(sym) =
                extract_named_symbol(node, source, file_path, SymbolKind::Method, "name")
            {
                symbols.push(sym);
            }
        }
        "type_declaration" => {
            // type_declaration は子として type_spec を持つ。
        }
        "type_spec" => {
            if let Some(sym) =
                extract_named_symbol(node, source, file_path, SymbolKind::Type, "name")
            {
                // 構造体かインターフェースかを判定する。
                let kind = node
                    .child_by_field_name("type")
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
            if let Some(sym) =
                extract_named_symbol(node, source, file_path, SymbolKind::Const, "name")
            {
                symbols.push(sym);
            }
        }
        "var_spec" => {
            if let Some(sym) =
                extract_named_symbol(node, source, file_path, SymbolKind::Static, "name")
            {
                symbols.push(sym);
            }
        }
        _ => {}
    }
}
