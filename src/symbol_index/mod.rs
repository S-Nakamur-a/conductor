//! コードナビゲーション用の tree-sitter ベースのシンボルインデックス。
//!
//! tree-sitter で Rust ソースファイルを構文解析してシンボル定義を抽出し、
//! 定義/実装/参照のルックアップを提供する SymbolIndex を提供する。
//!
//! 責務ごとに分割している。[model] は公開データ型（Symbol、SymbolKind、
//! Reference）、[index] は SymbolIndex 本体とその構築/クエリメソッド、
//! extract_common は各言語共通の tree-sitter AST 走査ヘルパー、
//! extract_rust/extract_go/extract_ts は言語ごとのシンボル抽出器を持つ。
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

pub use code_mask::{CodeMask, identifier_occurrences};
pub(crate) use code_mask::language_for_ext;
pub use index::SymbolIndex;
// Reference は現在 app/ から crate::symbol_index::Reference として外部利用されている。
// Symbol/SymbolKind は今のところこのモジュールツリー内で super::model::X としてのみ
// 使われているが、分割前の crate::symbol_index::X というパスを将来の呼び出し元のために
// 維持すべく、モジュールルートで再エクスポートしたままにしてある。
#[allow(unused_imports)]
pub use model::{Reference, Symbol, SymbolKind};
