//! ログのエントリ列を、幅に合わせた表示行に組み直す。
//!
//! 各ブロックは 2 カラムの溝を持つ。溝を除いた width - MARKER_COLS で markdown を
//! 描くので「論理行 1 つ = 表示行 1 つ」が保たれ、スクロールの上限が total - height で
//! 素直に決まる。ユーザのターンだけは markdown を通さず全幅の背景ブロックになる。

use conductor_core::claude_log::{DisplayBlock, LogEntry, Role};
use conductor_core::theme::Theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::markdown::{self, Flavor};
use crate::panels::viewer::syntax::Highlighter;

use super::style::{
    ASSISTANT_MARKER, INACTIVE, MARKER_COLS, Styles, TEAMMATE_GLYPH, THINKING_GLYPH, USER_MARKER,
    fit_glyph_line, fit_styled_line, is_width_ambiguous, markdown_theme, pad_glyph_to, with_marker,
};
use super::tool::{
    count_buckets, render_annotation, render_result_collapsed, render_result_expanded,
    render_tool_use,
};
use super::wrap::render_user_text;

/// /compact が文脈を切った位置に出る行。再開したトランスクリプトから採取した文言。
/// 宣伝している ctrl+o はこのビュー自体が履歴なので実現しないが、画面と揃える。
const COMPACT_BOUNDARY: &str = "Conversation compacted (ctrl+o for history)";

/// エントリ間の空行を表すブロック番号。
const SEPARATOR: usize = usize::MAX;

/// 溝のマーカーが始まりうる最も右のカラム。ここから先は本文なので、同じ字が出ても内容。
pub(super) const MAX_GUTTER_COL: usize = 2;

/// 描いた 1 行の出どころと、溝のために未書き込みで残すカラム。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct LineMeta {
    pub entry: usize,
    pub block: usize,
    /// ブロック内での行番号。幅を変えて組み直しても同じブロックの同じ位置に戻れる。
    pub offset: usize,
    pub skip_col: Option<u16>,
}

pub(super) struct Built {
    pub lines: Vec<Line<'static>>,
    pub meta: Vec<LineMeta>,
}

pub(super) struct Ctx<'a> {
    pub theme: &'a Theme,
    pub highlighter: &'a Highlighter,
    /// tool_use / tool_result / thinking を開くか。Claude Code の ctrl+o 相当。
    pub expanded: bool,
}

pub(super) fn build(ctx: &Ctx<'_>, entries: &[LogEntry], width: usize) -> Built {
    let styles = Styles::default();
    let md_theme = markdown_theme(ctx.theme);

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut meta: Vec<LineMeta> = Vec::new();

    for (ei, entry) in entries.iter().enumerate() {
        let before = lines.len();
        let buckets = count_buckets(entry);
        let mut summary_emitted = false;

        for (bi, block) in entry.blocks.iter().enumerate() {
            let pos = Pos {
                is_user: entry.role == Role::User,
                entry_has_content: lines.len() > before,
                buckets: &buckets,
            };
            let block_lines = render_block(
                ctx,
                &styles,
                &md_theme,
                width,
                &pos,
                block,
                &mut summary_emitted,
            );
            meta.extend((0..block_lines.len()).map(|offset| LineMeta {
                entry: ei,
                block: bi,
                offset,
                skip_col: None,
            }));
            lines.extend(block_lines);
        }

        // 注釈だけのエントリは上のエントリの続きであって別のターンではない。CLI は
        // スラッシュコマンドとその stdout を別レコードにするが、画面では地続きに描く。
        let next_is_continuation = entries.get(ei + 1).is_some_and(annotation_only);
        if !next_is_continuation && lines.len() > before {
            lines.push(Line::from(""));
            meta.push(LineMeta {
                entry: ei,
                block: SEPARATOR,
                offset: 0,
                skip_col: None,
            });
        }
    }

    for (line, m) in lines.iter().zip(meta.iter_mut()) {
        m.skip_col = width_risk_hole(line);
    }
    debug_assert_eq!(lines.len(), meta.len());
    Built { lines, meta }
}

fn annotation_only(entry: &LogEntry) -> bool {
    !entry.blocks.is_empty()
        && entry
            .blocks
            .iter()
            .all(|b| matches!(b, DisplayBlock::Annotation { .. }))
}

/// ⏺/⎿/✻ は unicode-width では 1 カラムだが多くの端末は 2 カラムで描く。直後の 1 セルを
/// 未書き込みにしておくと ratatui の diff がそこで切れ、crossterm が絶対位置の MoveTo を
/// 出す (Claude Code 自身と同じ手口)。走査を溝で止めるのは本文にも同じ字が出るから。
fn width_risk_hole(line: &Line<'_>) -> Option<u16> {
    let mut col = 0usize;
    for span in &line.spans {
        for cluster in span.content.graphemes(true) {
            if col > MAX_GUTTER_COL {
                return None;
            }
            let w = UnicodeWidthStr::width(cluster);
            // 異体字セレクタ付きは既に 2 カラムと測れているので穴は要らない。開けると
            // 本文の先頭セルを空白にしてしまう。
            if w == 1 && cluster.chars().next().is_some_and(is_width_ambiguous) {
                return u16::try_from(col + w).ok();
            }
            col += w;
        }
    }
    None
}

struct Pos<'a> {
    is_user: bool,
    /// このエントリで既に何か描いたか。ユーザの複数本文の間の空行に使う。
    entry_has_content: bool,
    buckets: &'a std::collections::HashMap<conductor_core::claude_log::CountedBucket, usize>,
}

fn render_block(
    ctx: &Ctx<'_>,
    styles: &Styles,
    md_theme: &Theme,
    width: usize,
    pos: &Pos<'_>,
    block: &DisplayBlock,
    summary_emitted: &mut bool,
) -> Vec<Line<'static>> {
    let body_width = width.saturating_sub(MARKER_COLS);

    match block {
        DisplayBlock::Text(text) if pos.is_user => user_turn(pos, styles, text, width),
        DisplayBlock::Text(text) => with_marker(
            markdown(ctx, md_theme, text, body_width),
            ASSISTANT_MARKER,
            styles.assistant,
        ),
        DisplayBlock::ToolUse {
            name,
            input,
            errored,
        } => render_tool_use(name, input, *errored, ctx.expanded, width, &styles.tools)
            .into_iter()
            .collect(),
        DisplayBlock::ToolResult {
            lines, is_error, ..
        } if ctx.expanded => render_result_expanded(lines, *is_error, width, &styles.tools),
        DisplayBlock::ToolResult {
            kind,
            lines,
            is_error,
        } => render_result_collapsed(
            *kind,
            lines,
            *is_error,
            pos.buckets,
            summary_emitted,
            width,
            &styles.tools,
        ),
        DisplayBlock::Thinking {
            text,
            duration_secs,
        } => thinking(ctx, styles, md_theme, width, text, *duration_secs),
        DisplayBlock::TeammateMessage { id, body } => {
            teammate(ctx, styles, md_theme, width, id, body)
        }
        DisplayBlock::Annotation { lines } => render_annotation(lines, width, &styles.tools),
        // 実測: タスク通知はツールの緑ではなくアシスタントと同じ丸印で描かれる。
        DisplayBlock::Notice(text) => vec![fit_glyph_line(
            ASSISTANT_MARKER,
            &[(text.clone(), styles.assistant)],
            width,
        )],
        DisplayBlock::CompactBoundary => vec![fit_glyph_line(
            THINKING_GLYPH,
            &[(COMPACT_BOUNDARY.to_string(), styles.result)],
            width,
        )],
    }
}

fn markdown(ctx: &Ctx<'_>, md_theme: &Theme, text: &str, body_width: usize) -> Vec<Line<'static>> {
    markdown::render(
        text,
        body_width,
        md_theme,
        ctx.highlighter.syntax_set(),
        ctx.highlighter.theme(),
        Flavor::Transcript,
    )
}

/// 実測: 同じエントリの 2 つ目の本文も独立したターンとして空行を挟んで描かれる。
/// 区切りはエントリの間にしか入らないので、この空行はここで入れる。
fn user_turn(pos: &Pos<'_>, styles: &Styles, text: &str, width: usize) -> Vec<Line<'static>> {
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

fn thinking(
    ctx: &Ctx<'_>,
    styles: &Styles,
    md_theme: &Theme,
    width: usize,
    text: &str,
    duration_secs: u64,
) -> Vec<Line<'static>> {
    if !ctx.expanded {
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
    let body = markdown(ctx, md_theme, text, width.saturating_sub(MARKER_COLS))
        .into_iter()
        .map(|mut line| {
            for span in &mut line.spans {
                span.style = Style::default().fg(INACTIVE).add_modifier(Modifier::ITALIC);
            }
            line
        })
        .collect();
    lines.extend(with_marker(body, " ", styles.thinking));
    lines
}

/// 折りたたみは 1 行で背景ブロック無し。thinking と違い普通のチャットなので本文は markdown。
fn teammate(
    ctx: &Ctx<'_>,
    styles: &Styles,
    md_theme: &Theme,
    width: usize,
    id: &str,
    body: &str,
) -> Vec<Line<'static>> {
    let marker = pad_glyph_to(TEAMMATE_GLYPH, MARKER_COLS);
    if !ctx.expanded {
        return vec![Line::from(vec![
            Span::styled(marker, styles.result),
            Span::styled(
                format!("Message from @{id} (ctrl+o to expand)"),
                styles.result,
            ),
        ])];
    }
    let mut lines = vec![Line::from(vec![
        Span::styled(marker, styles.result),
        Span::styled(format!("Message from @{id}"), styles.result),
    ])];
    if body.trim().is_empty() {
        return lines;
    }
    let rendered = markdown(ctx, md_theme, body, width.saturating_sub(MARKER_COLS));
    lines.extend(with_marker(rendered, " ", styles.result));
    lines
}
