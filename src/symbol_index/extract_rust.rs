//! Rust のシンボル抽出: 関数、構造体、enum、トレイト、impl、および関連アイテム。

use super::extract_common::{extract_named_symbol, node_text, scope_within, walk_tree};
use super::model::{Symbol, SymbolKind};

pub(super) fn extract_rust_symbols(
    root: tree_sitter::Node,
    source: &str,
    file_path: &str,
    symbols: &mut Vec<Symbol>,
) {
    walk_tree(root, source, file_path, symbols, visit_rust_node);
}

/// 中にある宣言が外から引けなくなる器。関数やブロックの本体。
const LOCAL_SCOPES: [&str; 1] = ["block"];

fn visit_rust_node(
    node: tree_sitter::Node,
    source: &str,
    file_path: &str,
    symbols: &mut Vec<Symbol>,
) {
    let scope = scope_within(node, &LOCAL_SCOPES);
    match node.kind() {
        "function_item" | "function_signature_item" => {
            if let Some(sym) =
                extract_named_symbol(node, source, file_path, SymbolKind::Function, scope, "name")
            {
                symbols.push(sym);
            }
        }
        "struct_item" => {
            if let Some(sym) =
                extract_named_symbol(node, source, file_path, SymbolKind::Struct, scope, "name")
            {
                symbols.push(sym);
            }
        }
        "enum_item" => {
            if let Some(sym) =
                extract_named_symbol(node, source, file_path, SymbolKind::Enum, scope, "name")
            {
                symbols.push(sym);
            }
        }
        "trait_item" => {
            if let Some(sym) =
                extract_named_symbol(node, source, file_path, SymbolKind::Trait, scope, "name")
            {
                symbols.push(sym);
            }
        }
        "impl_item" => {
            if let Some(type_node) = node.child_by_field_name("type") {
                let type_name = node_text(type_node, source).to_string();
                let line = node.start_position().row + 1;
                symbols.push(Symbol {
                    name: format!("impl {type_name}"),
                    kind: SymbolKind::Impl,
                    scope,
                    file_path: file_path.to_string(),
                    line,
                    parent: Some(type_name),
                });
            }
        }
        "type_item" => {
            if let Some(sym) =
                extract_named_symbol(node, source, file_path, SymbolKind::Type, scope, "name")
            {
                symbols.push(sym);
            }
        }
        "const_item" => {
            if let Some(sym) =
                extract_named_symbol(node, source, file_path, SymbolKind::Const, scope, "name")
            {
                symbols.push(sym);
            }
        }
        "static_item" => {
            if let Some(sym) =
                extract_named_symbol(node, source, file_path, SymbolKind::Static, scope, "name")
            {
                symbols.push(sym);
            }
        }
        "macro_definition" => {
            if let Some(sym) =
                extract_named_symbol(node, source, file_path, SymbolKind::Macro, scope, "name")
            {
                symbols.push(sym);
            }
        }
        "mod_item" => {
            if let Some(sym) =
                extract_named_symbol(node, source, file_path, SymbolKind::Module, scope, "name")
            {
                symbols.push(sym);
            }
        }
        "enum_variant" => {
            if let Some(sym) = extract_named_symbol(
                node,
                source,
                file_path,
                SymbolKind::EnumVariant,
                scope,
                "name",
            ) {
                symbols.push(sym);
            }
        }
        "field_declaration" => {
            if let Some(sym) =
                extract_named_symbol(node, source, file_path, SymbolKind::Field, scope, "name")
            {
                symbols.push(sym);
            }
        }
        _ => {}
    }
}
