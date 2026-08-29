//! TypeScript/JavaScript のシンボル抽出: 関数、クラス、インターフェース、
//! 型エイリアス、enum、メソッド、変数宣言子。
//!
//! 見つけたものには外から引けるかどうか (Scope) を付ける。落とす判断は
//! 索引の側が持つ。

use super::extract_common::{extract_named_symbol, scope_within, walk_tree};
use super::model::{Symbol, SymbolKind};

pub(super) fn extract_ts_symbols(
    root: tree_sitter::Node,
    source: &str,
    file_path: &str,
    symbols: &mut Vec<Symbol>,
) {
    walk_tree(root, source, file_path, symbols, visit_ts_node);
}

/// 中にある宣言が外から引けなくなる器。関数やブロックの本体と、
/// 本体を持たずに束縛だけを作る for の頭。
const LOCAL_SCOPES: [&str; 3] = ["statement_block", "for_statement", "for_in_statement"];

fn visit_ts_node(
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
        "class_declaration" => {
            if let Some(sym) =
                extract_named_symbol(node, source, file_path, SymbolKind::Struct, scope, "name")
            {
                symbols.push(sym);
            }
        }
        "interface_declaration" => {
            if let Some(sym) = extract_named_symbol(
                node,
                source,
                file_path,
                SymbolKind::Interface,
                scope,
                "name",
            ) {
                symbols.push(sym);
            }
        }
        "type_alias_declaration" => {
            if let Some(sym) =
                extract_named_symbol(node, source, file_path, SymbolKind::Type, scope, "name")
            {
                symbols.push(sym);
            }
        }
        "enum_declaration" => {
            if let Some(sym) =
                extract_named_symbol(node, source, file_path, SymbolKind::Enum, scope, "name")
            {
                symbols.push(sym);
            }
        }
        "method_definition" => {
            if let Some(sym) =
                extract_named_symbol(node, source, file_path, SymbolKind::Method, scope, "name")
            {
                symbols.push(sym);
            }
        }
        "lexical_declaration" | "variable_declaration" => {
            // const Foo = ... / let bar = ... — 変数宣言子を抽出する。
        }
        "variable_declarator" => {
            if let Some(sym) =
                extract_named_symbol(node, source, file_path, SymbolKind::Const, scope, "name")
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
