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

/// タグの属性テキスト（タグ名とその閉じ > の間の部分文字列）から
/// attr="..." の値を取り出す。単純な文字列検索であり、汎用の属性パーサ
/// ではない。上の tag_inner も意図的に完全な XML/HTML パーサにしていないのと
/// 同じ方針。
fn attr_value<'a>(attrs: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("{name}=\"");
    let start = attrs.find(&needle)? + needle.len();
    let rest = &attrs[start..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

/// lead の先頭にある <teammate-message teammate_id="...">…</teammate-message>
/// ラッパー（Claude Code CLI の形式ではなく、Conductor 独自のマルチエージェント
/// 構造）をパースして (id, body) にする。ラッパーの summary 属性は存在しても
/// 常に無視し、展開時に表示される body だけがメッセージ本体を持つ。終了タグが
/// 無い場合は文字列の末尾まで取り込む点は tag_inner の規約と同じ。lead が
/// このタグで始まっていない場合、またはタグが壊れている場合（開始タグに
/// 閉じの > が無い、または teammate_id 属性が無い）は None を返し、
/// 呼び出し元はテキストを通常の文章として扱う経路にフォールバックする。
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

/// user のテキストブロックを、Claude Code の実際の UI が表示する内容
/// （Conductor 独自の <teammate-message> ラッパーについては、それ用に定義した
/// 表示ブロック）へ正規化する。
///
/// セッションログは、素の user ターンの中に CLI が特別に描画する（または
/// 全く描画しない）いくつかのラッパー形式を記録している。生のまま残すと、
/// reflow トランスクリプトがユーザが直前まで見ていた画面とまるで違うものに
/// なってしまう。
///
/// * <teammate-message teammate_id="...">…</teammate-message> — 別のエージェント
///   チームメイトからのメッセージ。DisplayBlock::TeammateMessage に畳み込む。
/// * <command-name>/foo</command-name>…<command-args>bar</command-args> —
///   スラッシュコマンドの呼び出し。CLI は "> /foo bar" として表示する。
/// * <local-command-stdout>…</local-command-stdout> — ローカルコマンドの
///   出力。ラップを外して表示する（ここでサニタイズする。生の ANSI を
///   含むことがあるため）。
/// * <task-notification>…</task-notification> — バックグラウンドタスクの
///   完了。メッセージ全体を通知の <summary> 行に畳み込む。
///
/// <system-reminder> はこのリストに *含まれない* — そのままにしている理由は
/// この関数の末尾にあるコメントを参照。
///
/// 表示できるものが何も残らなかった場合は None を返す。
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

/// Content の値を表示用ブロックへ変換し、2つの表層形式（素の文字列と型付き
/// ブロック配列）を同じフラットな表現へ正規化する。
///
/// is_user が真なら user ターンのラッパー正規化（スラッシュコマンド、
/// local-command の stdout、タスク通知。normalise_user_text 参照）を適用する。
/// assistant のテキストは手を加えない。これらのタグを正当に *引用* して
/// いる場合があるため。
///
/// tool_kinds は、セッション全体で共有する tool_use の id → ResultKind の
/// ペアリングマップ（session.rs 参照）。Counted カテゴリの tool_use
/// ブロックは自分の id でこの bucket を書き込み、tool_result ブロックは
/// その id を引いて復元する（Inline/Hidden な呼び出しは生のツール名を
/// ここでは保持していない ── 必要なら分類時点で log::debug! できる程度
/// ため None になる。分類時に対応する tool_use が見つからなかった場合
/// （ログが途中で切れている場合など）も None）。このマップは1回の呼び出し
/// より長く生き、セッション内の全レコードを通して引き回される。これに
/// より、あるレコードの tool_use を、後のレコードの tool_result から
/// 見つけられる。
///
/// errored_ids は、tool_result がエラーを報告した tool_use_id をセッション
/// 全体で事前スキャンした集合（session.rs::scan_errored_tool_use_ids 参照）。
/// tool_use は対応する tool_result に到達するより前に描画されるため必要になる。
///
/// thinking_duration_secs は、このレコードについてあらかじめ計算した
/// 「Thought for Ns」の値（session.rs::thinking_duration_secs 参照）で、
/// ここで見つかるすべての Thinking ブロックに適用する。1レコードは
/// 全コンテンツブロックに対して1つのタイムスタンプしか持たないため、
/// 1レコード内に複数の Thinking ブロックがあってもこの値を共有する。
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
                    Some(DisplayBlock::ToolUse { name, input, errored })
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
