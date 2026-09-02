//! 構文木からシンボル定義を拾う。言語ごとに違うのは、どのノード種別が
//! どの種類のシンボルかと、宣言を外から引けなくする器の名前だけ。

use super::language::Grammar;
use super::model::{Scope, Symbol, SymbolKind};

pub(super) fn extract(
    grammar: Grammar,
    root: tree_sitter::Node,
    source: &str,
    file_path: &str,
    symbols: &mut Vec<Symbol>,
) {
    let table = Table::of(grammar);
    // 再帰にしてノードごとに walk() を呼ぶと、そのたびにカーソルの確保が走り、
    // 抽出全体の約 30% を占めた。カーソル 1 本で歩く。
    let mut cursor = root.walk();
    loop {
        if let Some(symbol) = table.symbol_at(cursor.node(), source, file_path) {
            symbols.push(symbol);
        }
        if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return;
            }
        }
    }
}

struct Table {
    /// 中にある宣言が外から引けなくなる器。
    local_scopes: &'static [&'static str],
    kind_of: fn(tree_sitter::Node) -> Option<SymbolKind>,
}

impl Table {
    fn of(grammar: Grammar) -> Self {
        match grammar {
            Grammar::Rust => Table {
                local_scopes: &["block"],
                kind_of: rust_kind,
            },
            Grammar::Go => Table {
                local_scopes: &["block"],
                kind_of: go_kind,
            },
            Grammar::TypeScript | Grammar::Tsx => Table {
                local_scopes: &["statement_block", "for_statement", "for_in_statement"],
                kind_of: ts_kind,
            },
        }
    }

    fn symbol_at(&self, node: tree_sitter::Node, source: &str, file_path: &str) -> Option<Symbol> {
        let kind = (self.kind_of)(node)?;
        let scope = self.scope_of(node);
        let text = |n: tree_sitter::Node| source[n.byte_range()].to_string();
        let (name, line, parent) = match kind {
            SymbolKind::Impl => {
                let type_name = text(node.child_by_field_name("type")?);
                let line = node.start_position().row + 1;
                (format!("impl {type_name}"), line, Some(type_name))
            }
            _ => {
                let name_node = node.child_by_field_name("name")?;
                (text(name_node), name_node.start_position().row + 1, None)
            }
        };
        if name.is_empty() {
            return None;
        }
        Some(Symbol {
            name,
            kind,
            scope,
            file_path: file_path.to_string(),
            line,
            parent,
        })
    }

    fn scope_of(&self, node: tree_sitter::Node) -> Scope {
        let mut current = node.parent();
        while let Some(ancestor) = current {
            if self.local_scopes.contains(&ancestor.kind()) {
                return Scope::Local;
            }
            current = ancestor.parent();
        }
        Scope::Global
    }
}

fn rust_kind(node: tree_sitter::Node) -> Option<SymbolKind> {
    Some(match node.kind() {
        "function_item" | "function_signature_item" => SymbolKind::Function,
        "struct_item" => SymbolKind::Struct,
        "enum_item" => SymbolKind::Enum,
        "enum_variant" => SymbolKind::EnumVariant,
        "field_declaration" => SymbolKind::Field,
        "trait_item" => SymbolKind::Trait,
        "impl_item" => SymbolKind::Impl,
        "type_item" => SymbolKind::Type,
        "const_item" => SymbolKind::Const,
        "static_item" => SymbolKind::Static,
        "macro_definition" => SymbolKind::Macro,
        "mod_item" => SymbolKind::Module,
        _ => return None,
    })
}

fn go_kind(node: tree_sitter::Node) -> Option<SymbolKind> {
    Some(match node.kind() {
        "function_declaration" => SymbolKind::Function,
        "method_declaration" => SymbolKind::Method,
        "type_spec" => match node.child_by_field_name("type").map(|t| t.kind()) {
            Some("struct_type") => SymbolKind::Struct,
            Some("interface_type") => SymbolKind::Interface,
            _ => SymbolKind::Type,
        },
        "const_spec" => SymbolKind::Const,
        "var_spec" => SymbolKind::Static,
        _ => return None,
    })
}

fn ts_kind(node: tree_sitter::Node) -> Option<SymbolKind> {
    Some(match node.kind() {
        "function_declaration" => SymbolKind::Function,
        "class_declaration" => SymbolKind::Struct,
        "interface_declaration" => SymbolKind::Interface,
        "type_alias_declaration" => SymbolKind::Type,
        "enum_declaration" => SymbolKind::Enum,
        "method_definition" => SymbolKind::Method,
        "variable_declarator" => SymbolKind::Const,
        _ => return None,
    })
}
