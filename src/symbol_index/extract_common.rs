//! 言語ごとのシンボル抽出器（extract_rust、extract_go、extract_ts）が共有する
//! tree-sitter AST 走査ヘルパー。

use super::model::{Symbol, SymbolKind};

/// 各ノードに対して pre-order で visitor を呼ぶ汎用 AST ウォーカー。
///
/// 走査全体を通して1つの TreeCursor を使い回すため、再帰ではなく反復で実装している。
/// 再帰版はノードごとに node.walk() を呼んでおり、その1回1回がアロケーションを伴う。
/// このリポジトリでは数万個の短命なカーソルが生成され、抽出処理全体の約30%を占めていた。
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
        // 子がない場合は兄弟ノードが見つかるまで親を辿って登る。
        // 親がなくなったら node を根とする部分木を走り終えたということ。
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return;
            }
        }
    }
}

/// "name" フィールドを子に持つノードから、名前付きシンボルを抽出する。
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
    Some(Symbol {
        name,
        kind,
        file_path: file_path.to_string(),
        line,
        scope: None,
    })
}

/// tree-sitter ノードのテキスト内容を取得する。
pub(super) fn node_text<'a>(node: tree_sitter::Node, source: &'a str) -> &'a str {
    &source[node.byte_range()]
}
