//! ツール名の分類。ログパーサ（パース時に tool_use がどの結果側の集約
//! bucket に属するか解決する必要がある）と、reflow トランスクリプトの
//! レンダラ（classify を直接使って各 tool_use 行のレイアウトを決める）の
//! 両方が共有する。
//!
//! ここに実装したテーブルは推測ではない。Claude Code 自身のデフォルト
//! （ctrl+o を使わない）トランスクリプト出力を1ツールずつ生バイトで
//! キャプチャして再構築したもの。

use serde_json::Value;

/// Claude Code のデフォルトのトランスクリプトが、1つのツール呼び出しを
/// どう描画するか。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCategory {
    /// tool_result の位置で「{verb} {N} {noun} (ctrl+o to expand)」という
    /// 1行のサマリに畳み込まれる。tool_use 自体は何も描画しない。
    Counted(CountedBucket),
    /// tool_use の位置に、それ自身の ⏺ {display_name}({arg}) 行として
    /// 描画される。成功時は tool_result は何も描画しない。エラー時に結果の
    /// 行を描画するのはこのカテゴリだけ — Counted は is_error を完全に
    /// 無視する（実測: 失敗した Read でも普通の「Read 1 file」サマリに
    /// 畳み込まれる）。reflow レンダラ参照。
    Inline {
        display_name: String,
        arg: Option<String>,
    },
    /// tool_use・tool_result のどちらの位置でも何も描画しない（TodoWrite
    /// など）。エラー時も同様 — このカテゴリについてエラー時は未実測だが、
    /// Claude Code 自身の UI は TodoWrite の失敗を可視のトランスクリプト
    /// テキストとして出したことがない。
    Hidden,
}

/// ToolCategory::Counted な呼び出しが属する、結果側の集約 bucket。1つの
/// エントリ内で同じ bucket に解決された2つのツール呼び出しは、1つの
/// 折りたたみ行に合算される — 元のテーブルによれば、これが cat の
/// シェル呼び出しが Read ツール呼び出しと合流する仕組み。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CountedBucket {
    Read,
    Search,
    List,
}

impl CountedBucket {
    /// 折りたたみサマリ行のための (verb, noun_singular, noun_plural)。
    /// 例えば ("Read", "file", "files") → "Read 3 files (ctrl+o to expand)"。
    pub fn labels(self) -> (&'static str, &'static str, &'static str) {
        match self {
            CountedBucket::Read => ("Read", "file", "files"),
            CountedBucket::Search => ("Searched for", "pattern", "patterns"),
            CountedBucket::List => ("Listed", "directory", "directories"),
        }
    }
}

/// 1つのエントリが複数の節を生成するときの、bucket の出現順。
/// 実測: ls×2 + Grep + Read は
/// 「Searched for 1 pattern, read 1 file, listed 2 directories」と描画される。
pub const BUCKET_ORDER: [CountedBucket; 3] = [
    CountedBucket::Search,
    CountedBucket::Read,
    CountedBucket::List,
];

/// ツール呼び出しの *結果* 側が何を描画するか — ToolCategory のうち
/// crate::claude_log::DisplayBlock::ToolResult まで残る半分。
///
/// Inline と Hidden はここで区別できる状態を保つ必要がある。失敗した
/// Inline の呼び出しは ⎿ Error: ブロックを描画するが、失敗した Hidden の
/// 呼び出しは何も描画しない（実測: is_error 付きの TodoWrite は出力を
/// 1行も生成しなかった）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultKind {
    Counted {
        bucket: CountedBucket,
        /// bucket 自身のツールが呼ばれたのではなく、シェル呼び出し
        /// （ls/cat）がこの bucket に分類された場合に true。ネイティブ側は
        /// これを *フォールバック* としてしかカウントしない。BUCKET_ORDER と
        /// レンダラの集約処理を参照。
        from_bash: bool,
    },
    Inline,
    Hidden,
}

/// classify の結果側への射影。
pub fn result_kind(name: &str, input: &Value) -> ResultKind {
    let from_bash = name == "Bash";
    match classify(name, input) {
        ToolCategory::Counted(bucket) => ResultKind::Counted { bucket, from_bash },
        ToolCategory::Inline { .. } => ResultKind::Inline,
        ToolCategory::Hidden => ResultKind::Hidden,
    }
}

/// tool_use 呼び出しを、生の API 名と input JSON から分類する。
///
/// カテゴリが引数に依存する唯一のツールが Bash: コマンドの最初の
/// 空白区切りの単語が ls なら CountedBucket::List、cat なら
/// CountedBucket::Read（Read ツール自身の bucket と合流）、それ以外なら
/// 普通の Bash(command) インライン行になる。
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

/// 指定した1つの引数キーを読んで Inline カテゴリを組み立てる。値が空また
/// は無い場合は arg: None にし、レンダラが括弧ごと省略できるようにする
/// （⏺ Name() ではなく ⏺ Name）。
///
/// 値はツールの *出力* と同様にサニタイズする。引数は生の JSON であり
/// （複数行の Bash コマンドやタブを含むパターンなど）、1つの表示範囲の中に
/// 改行やタブがあると端末と ratatui 自身のカラム計算がずれ、行が異常に
/// 幅広く表示されてしまう。
fn inline(display_name: &str, input: &Value, key: &str) -> ToolCategory {
    let arg = input
        .get(key)
        .and_then(Value::as_str)
        .map(super::convert::sanitize_preview_line)
        .filter(|s| !s.is_empty());
    ToolCategory::Inline {
        display_name: display_name.to_string(),
        arg,
    }
}

/// 既知のテーブルに無いツール名について、この固定の優先順で試す
/// 引数キーの探索順（元のテーブルの「未知ツールの引数キー探索順」）。
const UNKNOWN_ARG_KEYS: &[&str] = &[
    "command",
    "file_path",
    "path",
    "pattern",
    "url",
    "query",
    "description",
];

/// 未知のツールについて、UNKNOWN_ARG_KEYS を順に試し、最初に存在する
/// 空でない文字列引数を見つける。reflow レンダラの展開表示でも使い回す。
/// そこでは折りたたみ表示のカテゴリに関わらず、あらゆるツール
/// （Read/Grep のような Counted なものも含む）に *何らかの* 引数を表示する
/// 必要があるため。
pub fn unknown_tool_arg(input: &Value) -> Option<String> {
    let obj = input.as_object()?;
    for key in UNKNOWN_ARG_KEYS {
        if let Some(s) = obj.get(*key).and_then(Value::as_str) {
            let cleaned = super::convert::sanitize_preview_line(s);
            if !cleaned.is_empty() {
                return Some(cleaned);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn read_is_counted_bucket_read() {
        let input = json!({"file_path": "/a.txt"});
        assert_eq!(classify("Read", &input), ToolCategory::Counted(CountedBucket::Read));
    }

    #[test]
    fn grep_and_glob_are_counted_bucket_search() {
        let input = json!({"pattern": "foo"});
        assert_eq!(classify("Grep", &input), ToolCategory::Counted(CountedBucket::Search));
        assert_eq!(classify("Glob", &input), ToolCategory::Counted(CountedBucket::Search));
    }

    #[test]
    fn bash_ls_is_counted_bucket_list() {
        let input = json!({"command": "ls -la /tmp"});
        assert_eq!(classify("Bash", &input), ToolCategory::Counted(CountedBucket::List));
    }

    #[test]
    fn bash_cat_merges_into_counted_bucket_read() {
        // 元のテーブルでは cat のシェル呼び出しは Read ツール自身の bucket
        // に合流する — どちらも1つの "Read N files" 行にカウントされる。
        let input = json!({"command": "cat foo.txt"});
        assert_eq!(classify("Bash", &input), ToolCategory::Counted(CountedBucket::Read));
    }

    #[test]
    fn bash_other_command_is_inline() {
        let input = json!({"command": "cargo build"});
        assert_eq!(
            classify("Bash", &input),
            ToolCategory::Inline {
                display_name: "Bash".to_string(),
                arg: Some("cargo build".to_string()),
            }
        );
    }

    #[test]
    fn bash_leading_whitespace_still_dispatches_on_first_word() {
        let input = json!({"command": "   ls /tmp"});
        assert_eq!(classify("Bash", &input), ToolCategory::Counted(CountedBucket::List));
    }

    #[test]
    fn write_is_inline_with_file_path_arg() {
        let input = json!({"file_path": "/tmp/out.txt", "content": "..."});
        assert_eq!(
            classify("Write", &input),
            ToolCategory::Inline {
                display_name: "Write".to_string(),
                arg: Some("/tmp/out.txt".to_string()),
            }
        );
    }

    #[test]
    fn edit_displays_as_update() {
        let input = json!({"file_path": "/tmp/out.txt"});
        assert_eq!(
            classify("Edit", &input),
            ToolCategory::Inline {
                display_name: "Update".to_string(),
                arg: Some("/tmp/out.txt".to_string()),
            }
        );
    }

    #[test]
    fn task_displays_as_agent_with_description_arg() {
        let input = json!({"description": "investigate bug", "prompt": "..."});
        assert_eq!(
            classify("Task", &input),
            ToolCategory::Inline {
                display_name: "Agent".to_string(),
                arg: Some("investigate bug".to_string()),
            }
        );
    }

    #[test]
    fn webfetch_displays_as_fetch_with_url_arg() {
        let input = json!({"url": "https://example.com"});
        assert_eq!(
            classify("WebFetch", &input),
            ToolCategory::Inline {
                display_name: "Fetch".to_string(),
                arg: Some("https://example.com".to_string()),
            }
        );
    }

    #[test]
    fn todowrite_is_hidden() {
        let input = json!({"todos": []});
        assert_eq!(classify("TodoWrite", &input), ToolCategory::Hidden);
    }

    #[test]
    fn unknown_tool_falls_back_to_generic_arg_key_search() {
        let input = json!({"query": "some search term"});
        assert_eq!(
            classify("WebSearch", &input),
            ToolCategory::Inline {
                display_name: "WebSearch".to_string(),
                arg: Some("some search term".to_string()),
            }
        );
    }

    #[test]
    fn inline_arg_absent_key_becomes_none() {
        let input = json!({"content": "..."}); // file_path キーが全く無い
        assert_eq!(
            classify("Write", &input),
            ToolCategory::Inline {
                display_name: "Write".to_string(),
                arg: None,
            }
        );
    }

    #[test]
    fn inline_arg_empty_string_becomes_none() {
        let input = json!({"file_path": ""});
        assert_eq!(
            classify("Write", &input),
            ToolCategory::Inline {
                display_name: "Write".to_string(),
                arg: None,
            }
        );
    }

    #[test]
    fn unknown_tool_arg_tries_keys_in_priority_order() {
        // UNKNOWN_ARG_KEYS では command の方が file_path より優先度が高い。
        let input = json!({"file_path": "/a", "command": "run me"});
        assert_eq!(unknown_tool_arg(&input), Some("run me".to_string()));
    }

    #[test]
    fn unknown_tool_arg_none_when_no_known_key_present() {
        let input = json!({"unrelated": "x"});
        assert_eq!(unknown_tool_arg(&input), None);
    }

}
