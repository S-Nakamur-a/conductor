//! TypeScript/JavaScript のシンボル抽出: 関数、クラス、インターフェース、
//! 型エイリアス、enum、メソッド、トップレベルの変数宣言子。

use super::extract_common::{extract_named_symbol, walk_tree};
use super::model::{Symbol, SymbolKind};

pub(super) fn extract_ts_symbols(
    root: tree_sitter::Node,
    source: &str,
    file_path: &str,
    symbols: &mut Vec<Symbol>,
) {
    walk_tree(root, source, file_path, symbols, visit_ts_node);
}

fn visit_ts_node(
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
        "class_declaration" => {
            if let Some(sym) =
                extract_named_symbol(node, source, file_path, SymbolKind::Struct, "name")
            {
                symbols.push(sym);
            }
        }
        "interface_declaration" => {
            if let Some(sym) =
                extract_named_symbol(node, source, file_path, SymbolKind::Interface, "name")
            {
                symbols.push(sym);
            }
        }
        "type_alias_declaration" => {
            if let Some(sym) =
                extract_named_symbol(node, source, file_path, SymbolKind::Type, "name")
            {
                symbols.push(sym);
            }
        }
        "enum_declaration" => {
            if let Some(sym) =
                extract_named_symbol(node, source, file_path, SymbolKind::Enum, "name")
            {
                symbols.push(sym);
            }
        }
        "method_definition" => {
            if let Some(sym) =
                extract_named_symbol(node, source, file_path, SymbolKind::Method, "name")
            {
                symbols.push(sym);
            }
        }
        "lexical_declaration" | "variable_declaration" => {
            // const Foo = ... / let bar = ... — 変数宣言子を抽出する。
        }
        "variable_declarator" => {
            if let Some(sym) =
                extract_named_symbol(node, source, file_path, SymbolKind::Const, "name")
            {
                symbols.push(sym);
            }
        }
        "export_statement" => {
            // 再帰は walk_tree 側で行われる。
        }
        _ => {}
    }
}
