//! `SymbolInformation.kind` の番号を意味に直す。
//!
//! 番号は scip crate が生成する enum とずれている。SCIP の Kind は途中で項目が
//! 挿入されて番号が振り直されており、scip 0.9 では関数が 24 だが、索引に書いて
//! あるのは 17 である。producer の癖ではなく、producer が揃って古い番号を
//! 書いていることによる (rust-analyzer と scip-go の索引を実際に読んで、
//! 両方が同じ体系であることを確かめた)。だから表は 1 つでよく、ツール名で
//! 分けない。載っていない番号は [`SymbolKind::Unknown`] にする — 当てにいって
//! 別の名前を出すより黙るほうがよい。
//!
//! 番号を書かない producer もある (scip-typescript は `kind` も
//! `signature_documentation` も出さず、宣言を `documentation` のコードブロックに
//! 入れる)。そちらは [`from_declaration`] で宣言の綴りから読む。

use crate::SymbolKind;

/// 索引が書いた番号を種別に直す。
///
/// `symbol` も見るのは、型を指す番号が型別名と impl ブロックの両方に付くため
/// (実測 109 件のうち 105 件が impl ブロック)。番号だけでは分けられない。
pub(crate) fn of(raw: i32, symbol: &str) -> SymbolKind {
    match raw {
        3 => SymbolKind::AssociatedType,
        8 => SymbolKind::Constant,
        11 => SymbolKind::Enum,
        12 => SymbolKind::EnumMember,
        15 => SymbolKind::Field,
        17 => SymbolKind::Function,
        21 => SymbolKind::Interface,
        // 26 は本体を持つメソッド、67 と 70 は interface / trait 側の宣言。
        // 呼ぶ側から見れば同じものなので分けない。
        26 | 67 | 70 => SymbolKind::Method,
        29 => SymbolKind::Module,
        35 => SymbolKind::Package,
        37 => SymbolKind::Parameter,
        44 => SymbolKind::SelfParameter,
        49 => SymbolKind::Struct,
        53 => SymbolKind::Trait,
        55 if is_impl_block(symbol) => SymbolKind::ImplBlock,
        55 => SymbolKind::TypeAlias,
        58 => SymbolKind::TypeParameter,
        61 => SymbolKind::Variable,
        // self を取らない関連関数。呼び出し側から見れば関数なので分けない。
        80 => SymbolKind::Function,
        82 => SymbolKind::Static,
        _ => SymbolKind::Unknown,
    }
}

/// 宣言の綴りから種別を読む。番号を書かない producer 用。
///
/// scip-typescript が `documentation` に入れる宣言は TypeScript の quickinfo
/// そのままで、`(method) greet(): string` のように種別が前置される。前置の無い
/// ものは宣言の先頭語で判る。先頭語だけを見る。綴りの途中に現れる語
/// (`function` を返す型など) を拾うと、別の種別に化ける。
pub(crate) fn from_declaration(declaration: &str) -> SymbolKind {
    let head = declaration.trim_start();
    // TypeScript の quickinfo は種別を括弧で前置する。
    if let Some(rest) = head.strip_prefix('(') {
        let (marker, _) = rest.split_once(')').unwrap_or((rest, ""));
        return match marker {
            "method" => SymbolKind::Method,
            "property" => SymbolKind::Field,
            "parameter" => SymbolKind::Parameter,
            "local var" | "local function" => SymbolKind::Variable,
            "getter" | "setter" => SymbolKind::Method,
            "alias" => SymbolKind::TypeAlias,
            "enum member" => SymbolKind::EnumMember,
            _ => SymbolKind::Unknown,
        };
    }
    match head.split_whitespace().next().unwrap_or("") {
        "function" => SymbolKind::Function,
        "class" => SymbolKind::Class,
        "struct" => SymbolKind::Struct,
        "interface" => SymbolKind::Interface,
        "enum" => SymbolKind::Enum,
        "type" => SymbolKind::TypeAlias,
        "namespace" | "module" | "package" => SymbolKind::Module,
        "const" => SymbolKind::Constant,
        "var" | "let" => SymbolKind::Variable,
        _ => SymbolKind::Unknown,
    }
}

/// impl ブロックの符号か。`impl#[Rough][SyntacticLayer]` のように角括弧で終わる。
/// 型別名は `store/Implements#` のように `#` で終わる。
fn is_impl_block(symbol: &str) -> bool {
    symbol.ends_with(']')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 索引が書いた番号を種別に直す() {
        // 対応は rust-analyzer と scip-go の実索引で確かめたもの。ツール名で表を
        // 分けると、片方でしか観測していない番号がもう片方で黙って Unknown になる。
        for (raw, symbol, want) in [
            (17, "demo/Run().", SymbolKind::Function),
            (15, "demo/Loud#Volume.", SymbolKind::Field),
            (61, "0", SymbolKind::Variable),
            (21, "demo/Greeter#", SymbolKind::Interface),
            (35, "demo/", SymbolKind::Package),
            (67, "demo/Greeter#Greet.", SymbolKind::Method),
            // 型を指す番号は型別名と impl ブロックの両方に付く。符号で分ける。
            (55, "store/Implements#", SymbolKind::TypeAlias),
            (
                55,
                "worktree_ops/impl#[WorktreeManager][Default]",
                SymbolKind::ImplBlock,
            ),
            (9999, "x", SymbolKind::Unknown),
            (0, "x", SymbolKind::Unknown),
        ] {
            assert_eq!(of(raw, symbol), want, "{raw} {symbol}");
        }
    }

    #[test]
    fn 番号を書かない索引は宣言の綴りから読む() {
        // scip-typescript が実際に書く形。
        for (declaration, want) in [
            ("(method) greet(): string", SymbolKind::Method),
            ("(property) volume: number", SymbolKind::Field),
            ("(parameter) g: Greeter", SymbolKind::Parameter),
            ("interface Greeter", SymbolKind::Interface),
            ("class Loud", SymbolKind::Class),
            ("function run(g: Greeter): number", SymbolKind::Function),
            ("var message: string", SymbolKind::Variable),
            (r#"module "greet.tsx""#, SymbolKind::Module),
            // 戻り値の型が function でも、その宣言はプロパティである。
            ("(property) handler: function", SymbolKind::Field),
            ("", SymbolKind::Unknown),
            ("なにか", SymbolKind::Unknown),
        ] {
            assert_eq!(from_declaration(declaration), want, "{declaration:?}");
        }
    }
}
