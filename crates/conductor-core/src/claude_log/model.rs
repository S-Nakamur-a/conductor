//! 生ログから正規化した、表示用の会話エントリ。

use super::tool_class::ResultKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LogEntry {
    pub role: Role,
    pub blocks: Vec<DisplayBlock>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DisplayBlock {
    /// Markdown 本文。
    Text(String),
    /// ツール呼び出し。どう描くかは描画側が [classify](super::classify) で決める。
    ToolUse {
        name: String,
        input: serde_json::Value,
        /// 対応する tool_result がエラーを報告したか。対応が無ければ false。
        errored: bool,
    },
    /// ツールが返した結果。
    ToolResult {
        /// ペアとなる tool_use から決まる。ペアが取れなければ Hidden。
        kind: ResultKind,
        lines: Vec<String>,
        is_error: bool,
    },
    /// アシスタントの思考。duration_secs は折りたたみ表示の「Thought for Ns」用で最小 1。
    Thinking { text: String, duration_secs: u64 },
    /// Conductor 独自の <teammate-message teammate_id="..."> ラッパーで届いた、
    /// 別エージェントからのメッセージ。
    TeammateMessage { id: String, body: String },
    /// 直前のブロックに付随する ⎿ 始まりの注釈。スラッシュコマンドの stdout や、
    /// CLI が会話に持ち込んだファイル。
    Annotation { lines: Vec<String> },
    /// モデルではなく CLI 自身が出す 1 行の ⏺ 通知 (バックグラウンドタスクの完了)。
    Notice(String),
    /// /compact でコンテキストが切られた箇所。
    CompactBoundary,
}
