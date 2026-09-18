//! 画面上の 1 語がコードなのか、コメントや文字列の中の地の文なのかを tree-sitter で判定する。
//!
//! 答えるのはそれだけで、名前で定義や参照を探すことはしない。名前の一致は位置から
//! 始まる問いの答えにならないので、索引が答えられない位置はそのまま答えられないとする。
//! 意味索引への繋ぎは semantic_index::bridge にある。

mod code_mask;
mod language;
#[cfg(test)]
mod tests;

pub use code_mask::{
    CodeMask, code_identifiers_on_line, identifier_occurrences, is_rust_keyword,
    occurrence_span_in_source,
};
pub use language::{Language, language_for_ext};
