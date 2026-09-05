//! Claude Code のセッションログ (.jsonl) のパーサ。
//!
//! 行単位で寛容に読み、[LogEntry] の平坦な列にする。Claude Code 自身の画面に出ない
//! レコード (サイドチェイン、isMeta、compact の要約、ジャーナル) はここで落とす。
//! 描画は持たない。tool_use をどう描くかは [classify] の答えを描画側が解釈する。

mod convert;
mod model;
mod sanitize;
mod schema;
mod session;
#[cfg(test)]
mod tests;
mod tool_class;

pub use model::{DisplayBlock, LogEntry, Role};
pub use session::{load_session, parse_jsonl};
pub use tool_class::{
    BUCKET_ORDER, CountedBucket, ResultKind, ToolCategory, classify, unknown_tool_arg,
};
