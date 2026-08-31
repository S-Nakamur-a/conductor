//! コードナビゲーション用の tree-sitter ベースのシンボルインデックス。
//!
//! tree-sitter で Rust ソースファイルを構文解析してシンボル定義を抽出し、
//! 定義/実装/参照のルックアップを提供する SymbolIndex を提供する。
//!
//! [code_mask] はインデックス側では答えられない問いに答える。画面上の
//! ある単語が本当にコードなのか、それともコメントや文字列の中の地の文なのか。
//! 名前のルックアップが意味を持つ前に、どのナビゲーションクエリもこれを
//! 必要とする。

mod code_mask;
mod extract_common;
mod extract_go;
mod extract_rust;
mod extract_ts;
mod index;
mod model;
#[cfg(test)]
mod tests;

pub(crate) use code_mask::language_for_ext;
pub use code_mask::{
    CodeMask, code_identifiers_on_line, identifier_occurrences, is_rust_keyword,
    occurrence_span_in_source,
};
pub use index::SymbolIndex;
// Reference は現在 app/ から crate::symbol_index::Reference として外部利用されている。
// Symbol/SymbolKind は今のところこのモジュールツリー内で super::model::X としてのみ
// 使われているが、分割前の crate::symbol_index::X というパスを将来の呼び出し元のために
// 維持すべく、モジュールルートで再エクスポートしたままにしてある。
#[allow(unused_imports)]
pub use model::{Reference, Symbol, SymbolKind};
