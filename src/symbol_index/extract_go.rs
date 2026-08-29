//! Go のシンボル抽出: 関数、メソッド、型宣言（構造体・インターフェース）、
//! 定数、変数。

use super::extract_common::{extract_named_symbol, scope_within, walk_tree};
use super::model::{Symbol, SymbolKind};

pub(super) fn extract_go_symbols(
    root: tree_sitter::Node,
    source: &str,
    file_path: &str,
    symbols: &mut Vec<Symbol>,
) {
    walk_tree(root, source, file_path, symbols, visit_go_node);
}

/// 中にある宣言が外から引けなくなる器。関数やブロックの本体。
const LOCAL_SCOPES: [&str; 1] = ["block"];

fn visit_go_node(
    node: tree_sitter::Node,
    source: &str,
    file_path: &str,
    symbols: &mut Vec<Symbol>,
) {
    let scope = scope_within(node, &LOCAL_SCOPES);
    match node.kind() {
        "function_declaration" => {
            if let Some(sym) =
                extract_named_symbol(node, source, file_path, SymbolKind::Function, scope, "name")
            {
                symbols.push(sym);
            }
        }
        "method_declaration" => {
            if let Some(sym) =
                extract_named_symbol(node, source, file_path, SymbolKind::Method, scope, "name")
            {
                symbols.push(sym);
            }
        }
        "type_declaration" => {
            // type_declaration は子として type_spec を持つ。
        }
        "type_spec" => {
            if let Some(sym) =
                extract_named_symbol(node, source, file_path, SymbolKind::Type, scope, "name")
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
                extract_named_symbol(node, source, file_path, SymbolKind::Const, scope, "name")
            {
                symbols.push(sym);
            }
        }
        "var_spec" => {
            if let Some(sym) =
                extract_named_symbol(node, source, file_path, SymbolKind::Static, scope, "name")
            {
                symbols.push(sym);
            }
        }
        _ => {}
    }
}
