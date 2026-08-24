//! トランスクリプト1ブロック分の描画と、そこで使う配色一式。
//!
//! super::build::build_lines から切り出してあるのは、ブロック種別ごとの分岐が
//! 「エントリを回す→ブロックを回す」の二重ループの内側にあって、早期脱出のために
//! ラベル付きブロックまで使う深さになっていたため。関数にすると、各腕は素直に
//! return できる。

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::claude_log::DisplayBlock;

use super::build::BuildCtx;
use super::glyphs::{
    ASSISTANT_MARKER, MARKER_COLS, TEAMMATE_MESSAGE_GLYPH, THINKING_GLYPH, USER_MARKER,
};
use super::helpers::{fit_glyph_line, fit_styled_line, pad_glyph_to, with_marker};
use super::palette;
use super::tool_lines::{
    ToolStyles, render_annotation, render_tool_result_collapsed, render_tool_result_expanded,
    render_tool_use,
};
use super::user_text::render_user_text;

/// /compact が文脈を切った位置に出る ✻ 行の文言（再開したトランスクリプトからそのまま
/// 採取）。Conductor はここが宣伝する ctrl+o を実現できない。リフロービュー自体が
/// スクロールバックであり、全履歴はキー操作の向こうではなくこの行の上にすでにあるためだが、
/// ユーザーが画面で見たものとの一致を優先して文言は変えていない。
const COMPACT_BOUNDARY_TEXT: &str = "Conversation compacted (ctrl+o for history)";

/// トランスクリプトの装飾に使う配色一式。
///
/// Claude の固定パレット（Color は Copy）から1度だけ組み立てて、全ブロックで使い回す。
pub(super) struct TranscriptStyles {
    pub assistant: Style,
    /// 実測: ユーザーのターンはコーラル色の > 接頭辞ではなく全幅の背景ブロックで表され、
    /// マーカーも本文もそのブロックの背景色を持つ。
    pub user_marker: Style,
    pub user_body: Style,
    pub result: Style,
    pub thinking: Style,
    pub tools: ToolStyles,
}

impl Default for TranscriptStyles {
    fn default() -> Self {
        let result = Style::default().fg(palette::INACTIVE);
        Self {
            assistant: Style::default().fg(palette::TEXT),
            user_marker: Style::default()
                .fg(palette::USER_MARKER_FG)
                .bg(palette::USER_BG),
            user_body: Style::default().fg(palette::USER_TEXT).bg(palette::USER_BG),
            result,
            thinking: result.add_modifier(Modifier::ITALIC),
            tools: ToolStyles {
                marker: Style::default().fg(palette::SUCCESS),
                marker_err: Style::default().fg(palette::ERROR),
                name: Style::default()
                    .fg(palette::TEXT)
                    .add_modifier(Modifier::BOLD),
                // ツールの引数は本文と同じ色で、薄くしない。実測の画面では
                // ⏺ Write(/tmp/out.txt) の (...) 部分が灰色ではなく本文と同色になっている。
                arg: Style::default().fg(palette::TEXT),
                result,
                result_err: Style::default().fg(palette::ERROR),
            },
        }
    }
}

/// 1ブロックを描くのに要る、そのブロック固有の位置情報。
pub(super) struct BlockPos<'a> {
    /// ctx.entries への添字。Markdown キャッシュのキーにも使う。
    pub entry: usize,
    /// エントリ内でのブロックの添字。
    pub block: usize,
    /// このエントリがユーザーのターンか。
    pub is_user: bool,
    /// このエントリで既に何か描かれているか。
    /// ユーザーの複数テキストブロックの間に空行を入れるかの判断に使う。
    pub entry_has_content: bool,
    /// 折りたたみ表示のための、バケットごとの事前集計。
    pub bucket_counts: &'a std::collections::HashMap<crate::claude_log::CountedBucket, usize>,
}

/// ブロック1つを行の列に変換する。何も描かないブロックでは空を返す。
pub(super) fn render_block(
    ctx: &BuildCtx<'_>,
    styles: &TranscriptStyles,
    md_theme: &crate::theme::Theme,
    width: usize,
    pos: &BlockPos<'_>,
    block: &DisplayBlock,
    summary_emitted: &mut bool,
) -> Vec<Line<'static>> {
    // マーカーの溝を除いた本文の幅。
    let body_width = width.saturating_sub(MARKER_COLS);
    let (ei, bi) = (pos.entry, pos.block);

    match block {
        DisplayBlock::Text(text) => {
            if pos.is_user {
                return render_user_turn(pos, styles, text, width);
            }
            let md_lines = markdown(ctx, md_theme, &format!("{ei}:{bi}"), text, body_width);
            with_marker(md_lines, ASSISTANT_MARKER, styles.assistant)
        }
        DisplayBlock::ToolUse {
            name,
            input,
            errored,
        } => render_tool_use(name, input, *errored, ctx.expanded, width, &styles.tools)
            .into_iter()
            .collect(),
        DisplayBlock::ToolResult {
            kind,
            lines,
            is_error,
        } => {
            if ctx.expanded {
                render_tool_result_expanded(lines, *is_error, width, &styles.tools)
            } else {
                render_tool_result_collapsed(
                    *kind,
                    lines,
                    *is_error,
                    pos.bucket_counts,
                    summary_emitted,
                    width,
                    &styles.tools,
                )
            }
        }
        DisplayBlock::Thinking {
            text,
            duration_secs,
        } => render_thinking(ctx, styles, md_theme, width, ei, bi, text, *duration_secs),
        DisplayBlock::TeammateMessage { id, body } => {
            render_teammate_message(ctx, styles, md_theme, width, ei, bi, id, body)
        }
        DisplayBlock::Annotation { lines } => render_annotation(lines, width, &styles.tools),
        // ⏺ {text} をアシスタント本文の色で描く。実測では、タスク通知はツールの緑ではなく
        // アシスタントのターンと同じ丸印で描かれている。正確な色相はバイト列の採取からは
        // 復元できないので、トランスクリプト内の位置から見てアシスタント色と判断している。
        DisplayBlock::Notice(text) => vec![fit_glyph_line(
            ASSISTANT_MARKER,
            &[(text.clone(), styles.assistant)],
            width,
        )],
        DisplayBlock::CompactBoundary => vec![fit_glyph_line(
            THINKING_GLYPH,
            &[(COMPACT_BOUNDARY_TEXT.to_string(), styles.result)],
            width,
        )],
    }
}

fn markdown(
    ctx: &BuildCtx<'_>,
    md_theme: &crate::theme::Theme,
    key: &str,
    text: &str,
    body_width: usize,
) -> Vec<Line<'static>> {
    ctx.cache.render_flavored(
        key,
        text,
        body_width,
        md_theme,
        ctx.syntax_set,
        ctx.syntect_theme,
        crate::ui::markdown::MarkdownFlavor::Transcript,
    )
}

/// ユーザーのテキストブロック。
///
/// 実測では、ユーザーメッセージのテキストブロックはそれぞれが独立したターンとして
/// 描かれ、間に空行が入る。プロンプトと後続の system-reminder を持つメッセージは
/// 1組に詰めた形ではなく、区切られた2つの ❯ ターンになる。エントリ間の区切りは
/// エントリのあいだにしか入らないので、この空行はここで入れる必要がある。
///
/// 本文は Markdown を通さない（実測）。解釈すべき散文ではなく生の入力だからである。
/// 共有のマーカー溝ではなく、自前で全幅の背景ブロックを持つ。
fn render_user_turn(
    pos: &BlockPos<'_>,
    styles: &TranscriptStyles,
    text: &str,
    width: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if pos.entry_has_content {
        lines.push(Line::from(""));
    }
    lines.extend(render_user_text(
        text,
        width,
        USER_MARKER,
        styles.user_marker,
        styles.user_body,
    ));
    lines
}

/// 思考ブロック。折りたたみ時は1行の要約、展開時は ✻ Thinking… の見出しと
/// 薄い斜体の本文。
#[allow(clippy::too_many_arguments)]
fn render_thinking(
    ctx: &BuildCtx<'_>,
    styles: &TranscriptStyles,
    md_theme: &crate::theme::Theme,
    width: usize,
    ei: usize,
    bi: usize,
    text: &str,
    duration_secs: u64,
) -> Vec<Line<'static>> {
    if !ctx.expanded {
        // 折りたたみ時: 字形なし、マーカー列までインデントした
        // 「  Thought for {N}s (ctrl+o to expand)」。全体を INACTIVE にし、
        // 時間の部分だけ太字にする。
        return vec![fit_styled_line(
            MARKER_COLS,
            &[
                ("Thought for ".to_string(), styles.result),
                (
                    format!("{duration_secs}s"),
                    styles.result.add_modifier(Modifier::BOLD),
                ),
                (" (ctrl+o to expand)".to_string(), styles.result),
            ],
            width,
        )];
    }

    let mut lines = vec![Line::from(vec![
        Span::styled(pad_glyph_to(THINKING_GLYPH, MARKER_COLS), styles.thinking),
        Span::styled("Thinking\u{2026}", styles.thinking),
    ])];
    if text.trim().is_empty() {
        return lines;
    }

    let body_width = width.saturating_sub(MARKER_COLS);
    let md_lines = markdown(ctx, md_theme, &format!("{ei}:{bi}:think"), text, body_width);
    // Markdown の出力を薄い斜体に塗り直し、溝の下にインデントする
    // （マーカーは空白なので字形は繰り返されない）。
    let dimmed = md_lines
        .into_iter()
        .map(|mut line| {
            for span in &mut line.spans {
                span.style = Style::default()
                    .fg(palette::INACTIVE)
                    .add_modifier(Modifier::ITALIC);
            }
            line
        })
        .collect();
    lines.extend(with_marker(dimmed, " ", styles.thinking));
    lines
}

/// チームメイトからのメッセージ (S4: Claude Code CLI の形式ではなく Conductor 独自)。
///
/// 折りたたみ時は 1 行、背景ブロックは無し — › の字形が INACTIVE で全部を担う。
/// 展開時は見出しからトグルのヒントを外し、本文を 2 列インデントして続ける。
/// 思考ブロックと違い普通のチャット内容なので、本文は Markdown で描く。
#[allow(clippy::too_many_arguments)]
fn render_teammate_message(
    ctx: &BuildCtx<'_>,
    styles: &TranscriptStyles,
    md_theme: &crate::theme::Theme,
    width: usize,
    ei: usize,
    bi: usize,
    id: &str,
    body: &str,
) -> Vec<Line<'static>> {
    let marker_prefix = pad_glyph_to(TEAMMATE_MESSAGE_GLYPH, MARKER_COLS);
    if !ctx.expanded {
        return vec![Line::from(vec![
            Span::styled(marker_prefix, styles.result),
            Span::styled(
                format!("Message from @{id} (ctrl+o to expand)"),
                styles.result,
            ),
        ])];
    }

    let mut lines = vec![Line::from(vec![
        Span::styled(marker_prefix, styles.result),
        Span::styled(format!("Message from @{id}"), styles.result),
    ])];
    if body.trim().is_empty() {
        return lines;
    }
    let body_width = width.saturating_sub(MARKER_COLS);
    let md_lines = markdown(
        ctx,
        md_theme,
        &format!("{ei}:{bi}:teammate"),
        body,
        body_width,
    );
    lines.extend(with_marker(md_lines, " ", styles.result));
    lines
}
