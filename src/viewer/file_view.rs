//! unified diff 表示の型定義。

use crate::diff_state::{DiffLineTag, InlineSegment};

/// unified diff 表示中の1エントリ。
#[derive(Debug, Clone)]
pub enum UnifiedDiffEntry {
    /// ハンク間の区切り（隠れている行がない場合に使う）。
    HunkSeparator { func_header: Option<String> },
    /// ハンク間で隠れている行を表す、展開可能なコンテキストブロック。
    ExpandableContext {
        /// 現在隠れている行数。
        hidden_count: usize,
        /// 新ファイル側で最初に隠れている行番号（1始まり）。
        new_line_start: usize,
        /// 新ファイル側で最後に隠れている行番号（1始まり、両端含む）。
        new_line_end: usize,
        /// 次のハンクの関数コンテキストヘッダ（併せて表示する）。
        func_header: Option<String>,
    },
    /// 1行分のエントリ（コンテキスト・追加・削除のいずれか）。
    Line {
        tag: DiffLineTag,
        /// 新ファイル側の行番号。Equal/Insert では Some、Delete では None。
        new_line_no: Option<usize>,
        /// この行のテキスト内容。
        content: String,
        /// 行内の変更セグメント（単語単位の diff）。
        inline_segments: Vec<InlineSegment>,
    },
}
