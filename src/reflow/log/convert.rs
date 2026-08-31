//! 生のセッションログレコードを表示用ブロックへ正規化する処理: ツールの要約、
//! ANSI/制御コードのサニタイズ、user ターンのラッパー形式
//! （<command-name>、<local-command-stdout>、<task-notification>）の処理。

use std::collections::{HashMap, HashSet};

use super::model::DisplayBlock;
use super::schema::{Block, Content, ToolResultContent};
use super::tool_class::{ResultKind, result_kind};

/// 生のツール出力の行から、端末の描画をずらしてしまう文字を取り除く: ANSI
/// エスケープシーケンス、タブ（スペースに展開する）、その他の C0/C1 制御
/// コード。ツール出力（コマンド結果、ファイルダンプ）は任意のテキストであり、
/// 素のタブは端末上ではタブストップまでカーソルを進めるが ratatui は1セルと
/// 数える。色のエスケープも端末上では幅ゼロだが ratatui にとってはバイト幅
/// を持つ。どちらも行の残りをずらしてトランスクリプトパネルを崩す。そこで
/// 代わりに、整形済みのプレーンテキストプレビューを描画する。
pub(super) fn sanitize_preview_line(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // ANSI エスケープ — テキストとして描画されないようシーケンス全体を捨てる。
            '\u{1b}' => match chars.peek() {
                Some('[') => {
                    chars.next();
                    // CSI: 0x40〜0x7E の終端バイトまで読み進める。
                    for cc in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&cc) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    // OSC: BEL または ST 終端（ESC \）まで読み進める。
                    while let Some(cc) = chars.next() {
                        if cc == '\u{07}' {
                            break;
                        }
                        if cc == '\u{1b}' {
                            if matches!(chars.peek(), Some('\\')) {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                // 単独の ESC やその他のエスケープ形式: 続く1バイトも捨てる。
                _ => {
                    chars.next();
                }
            },
            '\t' => out.push_str("    "),
            c if c.is_control() => {} // CR など他の制御コードは捨てる
            c => out.push(c),
        }
    }
    out
}

/// ToolResultContent を個々の出力行に分割し、1行ずつ安全に描画できるよう
/// サニタイズする（sanitize_preview_line を参照）。
pub(super) fn result_lines(content: &ToolResultContent) -> Vec<String> {
    match content {
        ToolResultContent::None => Vec::new(),
        ToolResultContent::Text(s) => s.lines().map(sanitize_preview_line).collect(),
        ToolResultContent::Blocks(blocks) => blocks
            .iter()
            .flat_map(|b| b.text.lines().map(sanitize_preview_line))
            .collect(),
    }
}

/// 開始タグが存在すれば、<{tag}> と </{tag}> の間のテキストを返す。
/// 終了タグが無い場合は文字列の末尾までを取り込む。
fn tag_inner<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let rest = &text[start..];
    Some(match rest.find(&close) {
        Some(end) => &rest[..end],
        None => rest,
    })
}

/// 単純な文字列検索で、汎用の属性パーサではない。tag_inner を完全な XML/HTML パーサに
/// していないのと同じ方針。
fn attr_value<'a>(attrs: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("{name}=\"");
    let start = attrs.find(&needle)? + needle.len();
    let rest = &attrs[start..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

/// summary 属性は常に無視し、展開時に見える body だけが本体を持つ。lead がこのタグで
/// 始まっていない、または壊れていれば None を返して通常の文章として扱わせる。
fn parse_teammate_message(lead: &str) -> Option<(String, String)> {
    const OPEN_PREFIX: &str = "<teammate-message";
    const CLOSE: &str = "</teammate-message>";
    if !lead.starts_with(OPEN_PREFIX) {
        return None;
    }
    let tag_end = lead.find('>')?;
    let id = attr_value(&lead[OPEN_PREFIX.len()..tag_end], "teammate_id")?;
    let rest = &lead[tag_end + 1..];
    let body = match rest.find(CLOSE) {
        Some(end) => &rest[..end],
        None => rest,
    };
    Some((id.to_string(), body.trim().to_string()))
}

/// ログは CLI が特別に描く (あるいは全く描かない) ラッパー形式を素の user ターンの中に
/// 記録している。生のまま出すと、直前まで見ていた画面と別物になる。畳むのは
/// teammate-message / command-name+args / local-command-stdout / task-notification の 4 つ。
/// <system-reminder> を残す理由はこの関数の末尾にある。何も残らなければ None。
fn normalise_user_text(text: String) -> Option<DisplayBlock> {
    // ラッパー形式が認識されるのはメッセージの先頭だけ（実際にそこに書かれる
    // ため）。ユーザがプロンプトの途中でこれらのタグに *言及* しているだけの
    // 場合はテキストに手を加えない。
    let lead = text.trim_start();
    if lead.starts_with("<teammate-message")
        && let Some((id, body)) = parse_teammate_message(lead)
    {
        return Some(DisplayBlock::TeammateMessage { id, body });
    }
    // 実測: <task-notification> はメッセージの先頭に限らず *どこにあっても*
    // マッチし、畳み込むとメッセージが持っていた他の内容はすべて捨てられる。
    // Claude Code はタグの位置もレコードの書き手も確認しない — 手動で貼り
    // 付けた画面ダンプでも CLI 自身の通知と全く同じように畳み込まれ、周りに
    // 打たれた文章も道連れになる。そのため、以下の先頭タグ系の処理より前に
    // これを実行し、メッセージが複数の summary を持つ場合は *最初の* もの
    // だけを採用する。
    //
    // 使える <summary> が無い場合は、生のテキストにフォールバックせず
    // メッセージ全体が消える — これもタグが無い場合と空の場合の両方で実測済み。
    if lead.contains("<task-notification>") {
        let summary = tag_inner(lead, "summary")
            .map(str::trim)
            .filter(|s| !s.is_empty())?;
        return Some(DisplayBlock::Notice(sanitize_preview_line(summary)));
    }
    // CLI は常に終了タグ付きで書く。メッセージの先頭に終了タグの無いものが
    // あれば、それはコマンドレコードではなくユーザの文章なので、
    // 「コマンド名」としてメッセージ全体を飲み込まず、そのままにしておく。
    if lead.starts_with("<command-name>")
        && lead.contains("</command-name>")
        && let Some(name) = tag_inner(lead, "command-name")
    {
        let args = tag_inner(lead, "command-args").unwrap_or("").trim();
        let display = if args.is_empty() {
            name.trim().to_string()
        } else {
            format!("{} {}", name.trim(), args)
        };
        return (!display.is_empty()).then_some(DisplayBlock::Text(display));
    }
    if lead.starts_with("<local-command-stdout>")
        && let Some(stdout) = tag_inner(lead, "local-command-stdout")
    {
        // 実測: コマンドの stdout は、それ単体の user ターンとしてではなく、
        // 上にある ❯ /command 行に続く ⎿ の継続として描画される。
        let lines: Vec<String> = stdout
            .trim()
            .lines()
            .map(sanitize_preview_line)
            .filter(|l| !l.trim().is_empty())
            .collect();
        return (!lines.is_empty()).then_some(DisplayBlock::Annotation { lines });
    }
    // <system-reminder> の範囲はあえて取り除かない: 実測したところ、
    // ターンのテキストにインラインで入っている場合も、それ単体のブロックと
    // して届く場合も、Claude Code はそのまま描画する。画面上で読み手が
    // 決して目にしないリマインダーの方は、代わりにレコードの isMeta
    // フラグで隠される（手元のコーパスにあるリマインダーのみのレコード
    // 11件中10件がこのフラグを持つ）。session.rs がここに到達する前に
    // それらをスキップしている。
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| DisplayBlock::Text(trimmed.to_string()))
}

/// [Content] の 2 つの表層形式 (素の文字列と型付きブロック配列) を、同じ平坦な
/// 表示ブロックの列へ正規化する。
///
/// assistant のテキストには手を加えない。ラッパーのタグを正当に引用している
/// ことがあるため、正規化は is_user のときだけ ([normalise_user_text])。
///
/// 引数のうち 3 つはセッション全体を見ないと決まらない (どれも session.rs が作る):
/// tool_kinds は tool_use の id と結果種別の対応で、1 回の呼び出しより長く生きる
/// ため、後のレコードの tool_result から前のレコードの tool_use を引ける。
/// errored_ids が事前スキャンなのは、tool_use が対応する結果より先に描かれるから。
/// thinking_duration_secs はレコードに 1 つしか無いので、複数の Thinking が
/// あっても同じ値を共有する。
pub(super) fn content_to_display_blocks(
    content: Content,
    is_user: bool,
    tool_kinds: &mut HashMap<String, ResultKind>,
    errored_ids: &HashSet<String>,
    thinking_duration_secs: u64,
) -> Vec<DisplayBlock> {
    let text_block = |text: String| -> Option<DisplayBlock> {
        if text.is_empty() {
            return None;
        }
        if is_user {
            normalise_user_text(text)
        } else {
            Some(DisplayBlock::Text(text))
        }
    };
    match content {
        Content::Text(s) => text_block(s).into_iter().collect(),
        Content::Blocks(blocks) => blocks
            .into_iter()
            .filter_map(|b| match b {
                Block::Text { text } => text_block(text),
                Block::Thinking { thinking } => Some(DisplayBlock::Thinking {
                    text: thinking,
                    duration_secs: thinking_duration_secs,
                }),
                Block::ToolUse { id, name, input } => {
                    let errored = !id.is_empty() && errored_ids.contains(&id);
                    if !id.is_empty() {
                        tool_kinds.insert(id, result_kind(&name, &input));
                    }
                    Some(DisplayBlock::ToolUse {
                        name,
                        input,
                        errored,
                    })
                }
                Block::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    // 対応の取れない result（ログが途中で切れている、id が
                    // 無い）は Hidden にフォールバックする。tool_use が無いと
                    // どのカテゴリに属するか分からず、Inline と決め打ちすると
                    // 余計なエラーブロックを出してしまう。
                    let kind = tool_kinds
                        .get(&tool_use_id)
                        .copied()
                        .unwrap_or(ResultKind::Hidden);
                    let lines = result_lines(&content);
                    Some(DisplayBlock::ToolResult {
                        kind,
                        lines,
                        is_error,
                    })
                }
                Block::Other => None,
            })
            .collect(),
    }
}
