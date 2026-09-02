//! tree-sitter によるシンボル索引。名前で定義・実装を引き、参照をテキスト検索する。
//!
//! sheaf-core の索引が答えられない位置がここに落ちてくる構文層で、
//! semantic_index::bridge がその繋ぎを担う。索引は名前しか根拠を持たないので、
//! 答えは言語を跨がず、関数の中の宣言は載せない。
//!
//! [CodeMask] は索引とは別の問いに答える。画面上のある単語がコードなのか、
//! コメントや文字列の中の地の文なのか。名前を引く前に、どのクエリもこれを通る。

mod code_mask;
mod extract;
mod index;
mod language;
mod model;
#[cfg(test)]
mod tests;

pub use code_mask::{
    CodeMask, code_identifiers_on_line, identifier_occurrences, is_rust_keyword,
    occurrence_span_in_source,
};
pub use index::SymbolIndex;
pub use language::{Language, language_for_ext, same_language};
pub use model::{Reference, Scope, Symbol, SymbolKind};
