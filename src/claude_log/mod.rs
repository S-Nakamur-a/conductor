//! Claude Code セッションログのパーサ。
//!
//! Claude Code の .jsonl セッションファイルを読み込み、reflow トランスクリプトビュー
//! で表示するために [LogEntry] のフラットなリストへ正規化する。
//!
//! パースは行単位かつ寛容に行う。壊れた行は黙ってスキップし、未知の type は無視し、
//! サイドチェインのレコードは除外する。入力の内容によらずこのモジュールは panic しない。
//!
//! 責務ごとにファイルを分割している。[schema] は JSONL スキーマと一対一の生の serde 型、
//! [model] は表示用の型、[convert] は生レコードを表示用ブロックへ正規化する処理、
//! [session] はファイル読み込みの公開 API ([load_session])を持つ。

mod convert;
mod model;
mod schema;
mod session;
mod tool_class;
#[cfg(test)]
mod tests;

pub use session::load_session;

// DisplayBlock と Role は ui/reflow_view.rs から、LogEntry は app/reflow.rs から、
// それぞれ crate::claude_log::X 経由で外部から参照される。残りの re-export（生スキーマの型）
// は現状このモジュールツリー内でのみ super::model::X / super::schema::X として使われて
// いるが、将来外部から使う場合に備えて分割前の crate::claude_log::X というパスを
// 維持するためにモジュールルートで re-export したままにしている。
#[allow(unused_imports)]
pub use model::{DisplayBlock, LogEntry, Role};
#[allow(unused_imports)]
pub use schema::{Block, Content, LogRecord, Message, TextOnly, ToolResultContent};
// ToolCategory/CountedBucket/classify/unknown_tool_arg は ui/reflow_view/build.rs が
// tool_use/tool_result 行のレイアウトに使い、convert.rs（このモジュールツリー内）は
// ペアリングマップに格納された bucket を解決するのに使う。両方の呼び出し元が同じテーブルを
// 共有する理由は tool_class.rs を参照。
pub use tool_class::{
    BUCKET_ORDER, CountedBucket, ResultKind, ToolCategory, classify, unknown_tool_arg,
};
