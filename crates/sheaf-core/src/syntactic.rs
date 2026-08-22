//! 意味索引が答えられない位置を埋める層の境界。
//!
//! 実装は sheaf の外に置く。何を 1 つの語とみなすか、`format!("{x}")` のインライン引数を
//! どう扱うかは編集体験の一部で、sheaf が組み込む側に代わって決められない。
//! 言語ごとのパーサを抱えないためでもある。

use crate::{Location, Span};
use std::path::Path;

/// 位置にある語。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    /// 語がある。この範囲に収まっているメモだけが答えの候補になる。
    Word(Span),
    /// 語ではない。コメント、文字列の地の文、空白、記号。
    NotWord,
    /// 構文層が判定できない。索引は引かない。
    Unknown,
}

/// 構文層が返せる答え。
///
/// 意味索引由来であることを表す [`crate::Definition::Exact`] をここから作れないようにしてある。
/// 中心的な保証を、組み込み側が読むとは限らないドキュメントに預けないため。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyntacticAnswer {
    /// 構文レベルで探した結果。0 件でもよい。
    Found(Vec<Location>),
    /// その位置に識別子が無い。
    NotCode,
}

pub trait SyntacticLayer {
    /// その位置にある語の範囲を答える。
    ///
    /// 範囲は語ちょうどにする。広く取ると、その中に収まる別の語の定義まで答えに混ざる
    /// （`self.foo` を1つの語として返すと `self` の束縛が候補に入る）。
    fn token_at(&self, path: &Path, line: u32, col: u32) -> Token;

    /// その位置の定義候補を構文レベルで答える。索引が答えられなかったときに呼ばれる。
    fn definition_at(&self, path: &Path, line: u32, col: u32) -> SyntacticAnswer;

    /// その位置の識別子への参照を構文レベルで探す。索引が答えられなかったときに呼ばれる。
    ///
    /// 既定の実装は置かない。「探していない」を「見つからなかった」として返すと、
    /// 0 件が答えとして読まれる。
    ///
    /// ツリー全体を歩くので重い。tree-sitter による実装の実測で、`new` のような
    /// ありふれた名前で 200 ファイル約 157ms かかる。描画のたびに呼ぶ経路に置かないこと。
    fn references_at(&self, path: &Path, line: u32, col: u32) -> SyntacticAnswer;
}
