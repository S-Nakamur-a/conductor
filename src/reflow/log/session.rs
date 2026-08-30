//! ファイル読み込みの公開 API: セッションログをパースして表示用エントリにする。

use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::convert::content_to_display_blocks;
use super::model::{DisplayBlock, LogEntry, Role};
use super::schema::{Block, Content, LogRecord};
use super::tool_class::ResultKind;

/// Claude Code の .jsonl セッションファイルをパースし、表示用エントリを返す。
///
/// 壊れた行と未知のレコード種別は黙ってスキップする。
/// サイドチェインのレコード（isSidechain == true）は除外する。
/// ファイルの内容によらずこの関数は panic しない。
pub fn load_session(path: &Path) -> Vec<LogEntry> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            log::warn!("reflow: cannot read session file {}: {e}", path.display());
            return Vec::new();
        }
    };

    let records: Vec<LogRecord> = text
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            match serde_json::from_str(line) {
                Ok(r) => Some(r),
                Err(e) => {
                    log::warn!("reflow: skipping malformed jsonl line: {e}");
                    None
                }
            }
        })
        .collect();

    // 事前スキャン: tool_use のマーカーは、対応する tool_result がエラーを
    // 報告していたときエラー色で描画する必要があるが、その result レコードは
    // ログ上では必ず呼び出しの *後* に来る（連続する assistant/user レコードに
    // 分かれて入る）。そこでこの最初のパスでエラーだった tool_use_id を
    // すべて集めておき、下のエントリ構築パスが先読みせずに都度参照できる
    // ようにしている。
    let errored_ids = scan_errored_tool_use_ids(&records);

    let mut entries = Vec::new();
    // tool_use の id → その Counted bucket（Inline/Hidden な呼び出しは
    // 分類が終わると生のツール名を保持しないため、ここには挿入しない）。
    let mut tool_kinds: HashMap<String, ResultKind> = HashMap::new();
    // 直前に *表示された* レコードのタイムスタンプ。折りたたまれた Thinking
    // ブロックの「Thought for Ns」の秒数計算に使う。「直前」の定義は、
    // 実際に entries に入ったエントリを指す判断にしている。つまり
    // スキップされたレコード（isMeta/isSidechain/blocks が空/テキストの
    // 無いデキュー）は差分計算の基準にならない。スキップされた直後の
    // assistant ターンでも、隠れたターンではなくユーザが実際に見た最後の
    // ターンを基準に思考時間を測る。
    let mut prev_displayed_ts: Option<String> = None;
    for record in records {
        // CLI がユーザの代わりに注入したコンテキスト。2種類だけ ⎿ の1行を
        // 描画し、それ以外（hook 出力、skill 一覧など）は Claude Code 上で
        // 不可視であり、ここでも不可視のままにする。
        if record.kind == "attachment" {
            if let Some(text) = record.attachment.as_ref().and_then(attachment_line) {
                entries.push(LogEntry {
                    role: Role::User,
                    blocks: vec![DisplayBlock::Annotation { lines: vec![text] }],
                });
                // あえて prev_displayed_ts を進めない。attachment が持つ
                // タイムスタンプは、それを発行した compact のものであり、
                // 進めてしまうと次の assistant ターンの思考時間として
                // 誤って計上されてしまう。
            }
            continue;
        }

        // ✻ Conversation compacted — 描画される唯一の system レコード。
        if record.kind == "system" {
            if record.subtype.as_deref() == Some("compact_boundary") {
                entries.push(LogEntry {
                    role: Role::Assistant,
                    blocks: vec![DisplayBlock::CompactBoundary],
                });
            }
            continue;
        }

        // 会話を運ぶのは user と assistant のターンだけ。スキーマ上の他の
        // レコード種別はすべて Claude Code が一切描画しないセッション
        // メタデータのジャーナルである — queue-operation（入力キューの
        // enqueue/remove の記録）、mode、permission-mode、last-prompt、
        // ai-title、custom-title、agent-name、pr-link、
        // file-history-snapshot、file-history-delta。特に
        // queue-operation を実測したのは、これが一見表示できそうに *見える*
        // ためである。トップレベルの content に素の文字列としてキューされた
        // プロンプトを持っているが、それでも Claude Code は何も描画しない —
        // 処理中に入力されたプロンプトは、受理されると通常の user レコード
        // （promptSource: "queued"）として改めて発行されるので、ジャーナルの
        // 方も律儀に処理するとそのターンが二重に表示されてしまう。
        if record.kind != "user" && record.kind != "assistant" {
            continue;
        }
        if record.is_sidechain {
            continue;
        }
        // 隠しコンテキストの注入（skill ダンプ、caveat バナー、単独の
        // reminder）で、Claude Code 自身の UI では一切表示されないもの。
        if record.is_meta {
            continue;
        }
        // /compact のサマリは、次のコンテキストウィンドウに疑似 user ターン
        // として引き継がれる。Claude Code はこれを一切描画せず、代わりに
        // ⎿ Compacted (ctrl+o to see full summary) という行だけが立つ。
        // したがってここで本文を再生すると、ユーザが見たことのない文字列の
        // 壁がトランスクリプトに開いてしまう。
        if record.is_compact_summary {
            continue;
        }

        let this_ts = record.timestamp.clone();

        let Some(msg) = record.message else {
            continue;
        };

        let role = match msg.role.as_deref() {
            Some("user") => Role::User,
            Some("assistant") => Role::Assistant,
            _ => continue,
        };

        let duration_secs =
            thinking_duration_secs(prev_displayed_ts.as_deref(), this_ts.as_deref());

        let blocks = content_to_display_blocks(
            msg.content,
            role == Role::User,
            &mut tool_kinds,
            &errored_ids,
            duration_secs,
        );
        if blocks.is_empty() {
            continue;
        }

        entries.push(LogEntry { role, blocks });
        prev_displayed_ts = this_ts;
    }
    entries
}

/// attachment が描画する ⎿ 1行、または何も描画しない残り約27種別なら None。
///
/// 再開後のトランスクリプトで実測した形式:
/// ```text
///   ⎿  Read alpha.rs (42 lines)
///   ⎿  Referenced file beta.yml
/// ```
/// displayPath はそのまま使う。Claude Code 側で既にセッションの cwd に対する
/// 相対パスになっており、worktree の外のファイルに付く長い ../../.. の
/// プレフィックスも含めて済んでいるため。行数の無い file attachment は
/// 0 と表示せず、括弧の部分ごと省略する。
fn attachment_line(attachment: &super::schema::Attachment) -> Option<String> {
    let path = attachment
        .display_path
        .as_deref()
        .or(attachment.filename.as_deref())
        .map(str::trim)
        .filter(|p| !p.is_empty())?;
    match attachment.kind.as_str() {
        "file" => {
            let n = attachment
                .content
                .as_ref()
                .and_then(|c| c.file.as_ref())
                .and_then(|f| f.num_lines);
            Some(match n {
                Some(1) => format!("Read {path} (1 line)"),
                Some(n) => format!("Read {path} ({n} lines)"),
                None => format!("Read {path}"),
            })
        }
        "compact_file_reference" => Some(format!("Referenced file {path}")),
        _ => None,
    }
}

/// 折りたたまれた Thinking ブロックの「Thought for Ns」行のための、prev と
/// this の RFC3339 タイムスタンプの秒単位の差分。どちらかのタイムスタンプが
/// 無い/パースに失敗した場合、または計算結果がゼロ以下の場合（クロックの
/// ずれや、2レコードが同じ秒に収まった場合など）は 1 にフォールバックする
/// （仕様上 0 にはしない）。
fn thinking_duration_secs(prev: Option<&str>, this: Option<&str>) -> u64 {
    let (Some(prev), Some(this)) = (prev, this) else {
        return 1;
    };
    let (Ok(prev), Ok(this)) = (
        chrono::DateTime::parse_from_rfc3339(prev),
        chrono::DateTime::parse_from_rfc3339(this),
    ) else {
        return 1;
    };
    let diff = this.signed_duration_since(prev).num_seconds();
    if diff <= 0 { 1 } else { diff as u64 }
}

/// フィルタしていないレコード全体を対象に、対応する tool_result ブロックが
/// エラーを報告した tool_use_id をすべて集める。エントリ構築パスが
/// （後にある）tool_result レコードに到達する前に tool_use のマーカー色を
/// 解決するために使う。load_session の事前スキャンのコメントを参照。
fn scan_errored_tool_use_ids(records: &[LogRecord]) -> HashSet<String> {
    let mut ids = HashSet::new();
    for record in records {
        let Some(msg) = &record.message else { continue };
        let Content::Blocks(blocks) = &msg.content else {
            continue;
        };
        for block in blocks {
            if let Block::ToolResult {
                tool_use_id,
                is_error: true,
                ..
            } = block
            {
                ids.insert(tool_use_id.clone());
            }
        }
    }
    ids
}
