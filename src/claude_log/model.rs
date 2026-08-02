//! 生のセッションログスキーマから正規化した、表示用の型。

use super::tool_class::ResultKind;

/// 会話ターンの発話者。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

/// 生のログレコード1件から正規化した、表示用の会話エントリ。
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub role: Role,
    pub blocks: Vec<DisplayBlock>,
}

/// LogEntry 内の、表示用コンテンツの断片。
#[derive(Debug, Clone)]
pub enum DisplayBlock {
    /// Markdown 本文（ユーザ入力またはアシスタントのテキスト応答）。
    Text(String),
    /// ツール呼び出し。レンダラは name と生の input JSON を使ってこれを分類し
    /// （crate::claude_log::classify 参照）、行をどう描く／描かないか決める。
    ToolUse {
        name: String,
        input: serde_json::Value,
        /// この呼び出しに対応する tool_result がエラーを報告したかどうか。
        /// result レコードは必ず呼び出しの後に来るため、セッション全体を
        /// 事前スキャンして解決する（session.rs::scan_errored_tool_use_ids 参照）。
        /// 呼び出しに id が無い、または対応する result が見つからない場合
        /// （ログが途中で切れているなど）は false になる。
        errored: bool,
    },
    /// ツールが返した結果。Claude Code の折りたたまれた ⎿ ブロックを模す
    /// （Counted なツールの場合は結果側のカウントに畳み込まれる）。
    ToolResult {
        /// この結果が何を描画するかは、パース時にペアとなる tool_use から
        /// 解決する（Bash の分類が依存する呼び出しの input は、描画時点では
        /// もう残っていない）。ペアリングに失敗した場合は Hidden になる。
        /// 対応が取れない result はどのツールのものか分からず、そこに素の
        /// エラーブロックを描いてもノイズにしかならないため。
        kind: ResultKind,
        /// ツールの出力全文を1行ずつ格納したもの。以前の件数上限付きプレビュー
        /// と違い、展開表示には全行が必要になる。
        lines: Vec<String>,
        /// ツールがエラーを報告したかどうか。
        is_error: bool,
    },
    /// thinking ブロック — アシスタントの思考テキスト（空のこともある）。
    Thinking {
        text: String,
        /// 折りたたみ表示の「Thought for {N}s」に使う秒数。このレコードの
        /// タイムスタンプと、直前に表示されたレコードのタイムスタンプとの
        /// 秒単位の差分（session.rs 参照）で、最小値は1。
        duration_secs: u64,
    },
    /// 別のエージェントチームメイトからのメッセージ。ユーザターンの中に
    /// Conductor 独自の <teammate-message teammate_id="..."> ラッパーで
    /// 埋め込まれる（Claude Code CLI 自体の構造ではない。
    /// crate::claude_log::convert 参照）。ラッパーの summary 属性が
    /// あっても常に無視し、body がメッセージ全文で展開時のみ表示する。
    TeammateMessage { id: String, body: String },
    /// 直前のブロックに付随する ⎿ 始まりの注釈: スラッシュコマンドの出力
    /// （<local-command-stdout>）や、CLI が会話に持ち込んだファイル
    /// （file / compact_file_reference の添付）。
    ///
    /// 実測では、/model とその stdout は
    /// ```text
    /// ❯ /model
    ///   ⎿  Set model to Opus 5
    /// ```
    /// のように間に空行を挟まずに描画される。そのため、これだけで構成される
    /// エントリは、本来手前に入るはずの区切りを抑制する（ライン構築側を参照）。
    ///
    /// 複数行の stdout については実測できていないため、展開済みツール結果と
    /// 同じレイアウト（1行目にグリフ、以降5カラムインデント）にしている。
    Annotation { lines: Vec<String> },
    /// モデルではなく CLI 自身が生成した1行の ⏺ 通知 — 現状はバックグラウンド
    /// タスク完了のみで、その <task-notification> ラッパーは <summary> の
    /// テキストだけに畳み込まれる。
    ///
    /// 実測では、XML ラッパー全体が
    /// ⏺ Background command "…" completed (exit code 0)
    /// に置き換わる。
    Notice(String),
    /// /compact でコンテキストが切られた箇所に書かれる
    /// ✻ Conversation compacted マーカー。
    CompactBoundary,
}
