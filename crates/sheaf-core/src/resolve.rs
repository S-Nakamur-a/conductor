//! 意味層と構文層の切り替え。
//!
//! 索引が最新でも occurrence が存在しない位置がある（Rust の `format!("{x}")` の
//! インライン引数がそれで、実測でひとつも occurrence が無い一方、rust-analyzer は
//! 同じ位置で定義を返す）。したがって切り替えの判定は位置ごとに行う。

use crate::store::{Implemented, Resolved};
use crate::syntactic::{SyntacticAnswer, SyntacticLayer, Token};
use crate::{Definition, Enclosures, Implementations, References, Store, SymbolDetail};
use std::path::Path;

/// その位置にある語の定義を答える。
///
/// `rel` はソースツリーのルートからの相対パス。行・列はともに 0 始まり。
pub fn definition_at(
    store: &Store,
    syntactic: &dyn SyntacticLayer,
    rel: &Path,
    line: u32,
    col: u32,
) -> Definition {
    let abs = store.root().join(rel);
    match syntactic.token_at(&abs, line, col) {
        Token::NotWord => Definition::NotCode,
        Token::Word(span) => match store.definitions_in(rel, span) {
            Some(Resolved::Direct(locations)) => Definition::Exact(locations),
            Some(Resolved::Enclosing(found)) => Definition::Enclosing(found),
            // 索引に無い語と、内容が変わったファイルはここに落ちる。
            None => fall_back(syntactic, &abs, line, col),
        },
        Token::Unknown => fall_back(syntactic, &abs, line, col),
    }
}

/// その位置の語について、索引が書いている説明。所属・種別・宣言を見せるための
/// もので、どこにあるかの主張ではない。
///
/// 索引が最新でない、その位置に occurrence が無い、識別子ではない、のいずれでも
/// 空を返す。[`definition_at`] が [`Definition::Exact`] を返した位置でだけ使うこと --
/// 構文層に落ちた答えの横に索引由来の綴りを並べると、出どころの違う 2 つが
/// 1 つの説明に見える。
pub fn describe_at(
    store: &Store,
    syntactic: &dyn SyntacticLayer,
    rel: &Path,
    line: u32,
    col: u32,
) -> Vec<SymbolDetail> {
    let abs = store.root().join(rel);
    match syntactic.token_at(&abs, line, col) {
        Token::Word(span) => store.describe_in(rel, span).unwrap_or_default(),
        Token::NotWord | Token::Unknown => Vec::new(),
    }
}

/// その行を囲んでいるシンボル。内側が先。
///
/// 位置ではなく行だけで引くので構文層は要らない。
pub fn enclosures_at(store: &Store, rel: &Path, line: u32) -> Enclosures {
    match store.enclosures_in(rel, line) {
        Some(found) => Enclosures::Exact(found),
        None => Enclosures::Unknown,
    }
}

/// その位置の語が trait なら、それを実装している impl ブロック。
///
/// 索引が答えられなければ [`Implementations::Unknown`] を返す。ここだけは構文層に
/// 落とさない -- 実装の探索は名前の一致で当てにいく作業で、位置から始まる問いの
/// 答えとしては別物になる。落とすかどうかは呼び出し側が決める。
pub fn implementations_at(
    store: &Store,
    syntactic: &dyn SyntacticLayer,
    rel: &Path,
    line: u32,
    col: u32,
) -> Implementations {
    let abs = store.root().join(rel);
    match syntactic.token_at(&abs, line, col) {
        Token::NotWord => Implementations::NotCode,
        Token::Word(span) => match store.implementations_in(rel, span) {
            Some(Implemented::Declared(found)) => Implementations::Exact(found),
            Some(Implemented::Derived(found)) => Implementations::Derived(found),
            None => Implementations::Unknown,
        },
        Token::Unknown => Implementations::Unknown,
    }
}

fn fall_back(syntactic: &dyn SyntacticLayer, abs: &Path, line: u32, col: u32) -> Definition {
    match syntactic.definition_at(abs, line, col) {
        SyntacticAnswer::Found(locations) => Definition::Syntactic(locations),
        SyntacticAnswer::NotCode => Definition::NotCode,
    }
}

/// その位置にある語への参照を答える。
///
/// `rel` はソースツリーのルートからの相対パス。行・列はともに 0 始まり。
/// 索引が依拠するファイルのどれか 1 つでも変わっていれば構文層に回す。件数そのものが
/// 信用できなくなるので、残っている分だけを返すことはしない。
pub fn references_at(
    store: &Store,
    syntactic: &dyn SyntacticLayer,
    rel: &Path,
    line: u32,
    col: u32,
) -> References {
    let abs = store.root().join(rel);
    match syntactic.token_at(&abs, line, col) {
        Token::NotWord => References::NotCode,
        Token::Word(span) => match store.references_in(rel, span) {
            Some(found) => References::Exact(found),
            None => fall_back_references(syntactic, &abs, line, col),
        },
        Token::Unknown => fall_back_references(syntactic, &abs, line, col),
    }
}

fn fall_back_references(
    syntactic: &dyn SyntacticLayer,
    abs: &Path,
    line: u32,
    col: u32,
) -> References {
    match syntactic.references_at(abs, line, col) {
        SyntacticAnswer::Found(locations) => References::Syntactic(locations),
        SyntacticAnswer::NotCode => References::NotCode,
    }
}
