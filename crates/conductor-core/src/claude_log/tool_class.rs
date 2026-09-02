//! ツール呼び出しの分類。Claude Code 自身のデフォルト (ctrl+o 無し) のトランスクリプト
//! 出力を 1 ツールずつ生バイトで採取して再構築したテーブルで、推測は含まない。

use serde_json::Value;

use super::sanitize::sanitize_line;

/// Claude Code のデフォルトのトランスクリプトが、1 つのツール呼び出しをどう描くか。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCategory {
    /// tool_result の位置で「{verb} {N} {noun}」の 1 行に畳まれ、tool_use 自体は描かれない。
    /// is_error は無視される (失敗した Read も「Read 1 file」に畳まれる)。
    Counted(CountedBucket),
    /// tool_use の位置に ⏺ {display_name}({arg}) として描かれる。
    /// エラー時に結果の行を描くのはこのカテゴリだけ。
    Inline {
        display_name: String,
        arg: Option<String>,
    },
    /// どちらの位置でも描かれない (TodoWrite など)。エラー時も同様。
    Hidden,
}

/// Counted な呼び出しの集約先。1 エントリ内で同じ bucket に落ちた呼び出しは 1 行に合算される。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CountedBucket {
    Read,
    Search,
    List,
}

impl CountedBucket {
    /// 折りたたみ行の (動詞, 単数形, 複数形)。
    pub fn labels(self) -> (&'static str, &'static str, &'static str) {
        match self {
            CountedBucket::Read => ("Read", "file", "files"),
            CountedBucket::Search => ("Searched for", "pattern", "patterns"),
            CountedBucket::List => ("Listed", "directory", "directories"),
        }
    }
}

/// 複数の bucket が 1 行に並ぶときの順。実測: ls×2 + Grep + Read は
/// 「Searched for 1 pattern, read 1 file, listed 2 directories」。
pub const BUCKET_ORDER: [CountedBucket; 3] = [
    CountedBucket::Search,
    CountedBucket::Read,
    CountedBucket::List,
];

/// [ToolCategory] のうち tool_result 側に残る半分。
///
/// Inline と Hidden を区別したまま持つのは、失敗した Inline は ⎿ Error: を描き、
/// 失敗した Hidden は何も描かないため。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultKind {
    Counted {
        bucket: CountedBucket,
        /// bucket 自身のツールではなく、シェルの ls / cat がこの bucket に落ちた。
        from_bash: bool,
    },
    Inline,
    Hidden,
}

pub fn result_kind(name: &str, input: &Value) -> ResultKind {
    match classify(name, input) {
        ToolCategory::Counted(bucket) => ResultKind::Counted {
            bucket,
            from_bash: name == "Bash",
        },
        ToolCategory::Inline { .. } => ResultKind::Inline,
        ToolCategory::Hidden => ResultKind::Hidden,
    }
}

/// tool_use を API 名と input から分類する。引数で分類が変わるのは Bash だけ。
pub fn classify(name: &str, input: &Value) -> ToolCategory {
    match name {
        "Read" => ToolCategory::Counted(CountedBucket::Read),
        "Grep" | "Glob" => ToolCategory::Counted(CountedBucket::Search),
        "Bash" => classify_bash(input),
        "Write" => inline("Write", input, "file_path"),
        "Edit" => inline("Update", input, "file_path"),
        "Task" => inline("Agent", input, "description"),
        "WebFetch" => inline("Fetch", input, "url"),
        "TodoWrite" => ToolCategory::Hidden,
        other => ToolCategory::Inline {
            display_name: other.to_string(),
            arg: unknown_tool_arg(input),
        },
    }
}

fn classify_bash(input: &Value) -> ToolCategory {
    let command = input.get("command").and_then(Value::as_str).unwrap_or("");
    match command.split_whitespace().next() {
        Some("ls") => ToolCategory::Counted(CountedBucket::List),
        Some("cat") => ToolCategory::Counted(CountedBucket::Read),
        _ => inline("Bash", input, "command"),
    }
}

fn inline(display_name: &str, input: &Value, key: &str) -> ToolCategory {
    ToolCategory::Inline {
        display_name: display_name.to_string(),
        arg: string_arg(input, key),
    }
}

const UNKNOWN_ARG_KEYS: &[&str] = &[
    "command",
    "file_path",
    "path",
    "pattern",
    "url",
    "query",
    "description",
];

/// 既知のテーブルに無いツールの引数。UNKNOWN_ARG_KEYS の順で最初に見つかった空でない文字列。
pub fn unknown_tool_arg(input: &Value) -> Option<String> {
    UNKNOWN_ARG_KEYS
        .iter()
        .find_map(|key| string_arg(input, key))
}

fn string_arg(input: &Value, key: &str) -> Option<String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(sanitize_line)
        .filter(|s| !s.is_empty())
}
