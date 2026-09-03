//! Viewer の描画。行は [body] が組み、frame へ渡すのは最後の 1 手だけ。
//!
//! 中身の判断を [ratatui::Frame] の外に出しているので、テストは Line の並びを見る。

use conductor_core::diff_state::DiffLineTag;
use conductor_core::icons::IconSet;
use conductor_core::review_store::ReviewComment;
use conductor_core::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
};

use super::diff::{Entry, SideRow, side_by_side};
use super::tabs;
use super::thread;
use super::{Scroll, ViewerPanel};
use crate::review::ReviewState;
use crate::workspace::Workspace;

/// ガターのうち行番号が使わない列: 折りたたみマーカー(1) + 空白(1) + '│'(1) + 空白(1)。
/// diff 表示ではさらに +/- の 1 列と空白 1 列。
pub const GUTTER_FIXED: usize = 4;
const DIFF_SIGN: usize = 2;

/// 本文の上に載るタブ帯の高さ。[super::ViewerPanel::sync_layout] も同じ値を引く。
pub const TAB_ROW: u16 = 1;

/// 変更サマリのバナーが本文に譲るまでの行数。
const SUMMARY_ROWS: usize = 6;

/// コメントの印の桁。コメントのあるファイルでだけ確保する。
pub const MARK: usize = 2;

/// テスト実行ボタンの桁。実行できるテストのあるファイルでだけ確保する。
const BADGE: usize = 2;

/// 実行ボタンの桁を開けるか。当たり判定 (gutter_zone) も同じ答えを読む。
pub fn badge_width(panel: &ViewerPanel) -> usize {
    if panel.diff.active || panel.content.tests.is_empty() {
        0
    } else {
        BADGE
    }
}

/// markdown を折り返す幅。右端 1 桁はスクロールバーの軌道に譲る。
pub fn md_width(body: Rect) -> usize {
    (body.width as usize).saturating_sub(1).max(1)
}

/// 絵に使える升目。最終行は寸法とファイルサイズの情報行に譲る。
pub fn media_area(body: Rect) -> (u16, u16) {
    (body.width, body.height.saturating_sub(1))
}

const RAW: &str = "Raw";
const RENDERED: &str = "Rendered";

/// [Raw|Rendered] チップの桁数。
const CHIP: u16 = 1 + RAW.len() as u16 + 1 + RENDERED.len() as u16 + 1;

/// チップを出すのに要るタブ帯の幅。これより狭いとタブの余地が無くなるので出さない
/// (キーボードとパレットからは引き続き切り替えられる)。
const MIN_TAB_ROW: u16 = CHIP + 8;

/// タブ帯の右端に置く Raw / Rendered トグルの桁。
pub struct Toggle {
    /// 生ソースを選ぶ側 ([Raw)。
    pub raw: std::ops::Range<u16>,
    /// レンダリング表示を選ぶ側 (|Rendered])。
    pub rendered: std::ops::Range<u16>,
}

/// トグルの位置。描画も当たり判定もここだけを読むので、出ていないチップが
/// 押せることはない。
pub fn toggle(width: u16, available: bool) -> Option<Toggle> {
    if !available || width < MIN_TAB_ROW {
        return None;
    }
    let start = width - CHIP;
    let split = start + 1 + RAW.len() as u16;
    Some(Toggle {
        raw: start..split,
        rendered: split..width,
    })
}

fn toggle_spans(rendered: bool, theme: &Theme) -> Vec<Span<'static>> {
    let chrome = Style::default().fg(theme.muted);
    let on = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let pick = |active: bool| if active { on } else { chrome };
    vec![
        Span::styled("[", chrome),
        Span::styled(RAW, pick(!rendered)),
        Span::styled("|", chrome),
        Span::styled(RENDERED, pick(rendered)),
        Span::styled("]", chrome),
    ]
}

pub fn render(frame: &mut Frame, rect: Rect, ws: &Workspace) {
    let inner = crate::list::inner(rect);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let panel = &ws.panels.viewer;
    let strip = Rect {
        height: TAB_ROW,
        ..inner
    };
    frame.render_widget(
        Paragraph::new(tab_row(panel, &ws.theme, inner.width)),
        strip,
    );

    let body_area = Rect {
        y: inner.y + TAB_ROW,
        height: inner.height.saturating_sub(TAB_ROW),
        ..inner
    };
    let lines = body(
        panel,
        &ws.review,
        &ws.theme,
        ws.config.ui.icon_set(),
        body_area.width,
        body_area.height as usize,
    );
    frame.render_widget(Paragraph::new(lines), body_area);
    scrollbar(frame, body_area, panel);
    if let Some(hover) = &panel.nav.hover {
        popup(
            frame,
            super::hover::popup(hover, &ws.theme, body_area),
            &ws.theme,
        );
    }
}

/// ホバーのポップアップ。
fn popup(frame: &mut Frame, popup: super::hover::Popup, theme: &Theme) {
    frame.render_widget(Clear, popup.rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));
    let inner = block.inner(popup.rect);
    frame.render_widget(block, popup.rect);
    frame.render_widget(Paragraph::new(popup.body).wrap(Wrap { trim: false }), inner);
    for (rect, line) in popup.footer {
        frame.render_widget(Paragraph::new(line), rect);
    }
}

/// 畳んだぶんを除いた尺で出す。畳んだまま端まで送ったのにつまみが半分、が起きない。
fn scrollbar(frame: &mut Frame, area: Rect, panel: &ViewerPanel) {
    if panel.content.media.is_some() {
        return;
    }
    let (total, at) = if panel.diff.active {
        (panel.diff.entries.len(), panel.scroll.diff)
    } else if panel.is_showing_rendered_markdown() {
        (panel.content.rendered.len(), panel.scroll.md)
    } else {
        let total = panel.content.lines.len();
        (
            panel.fold.visible_count(total),
            panel.fold.visible_index(panel.scroll.line + 1, total),
        )
    };
    if total <= area.height as usize {
        return;
    }
    let mut state = ScrollbarState::new(total - area.height as usize).position(at);
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None),
        area,
        &mut state,
    );
}

/// タブ帯。開いていなければ未選択の案内を出す。
pub fn tab_row(panel: &ViewerPanel, theme: &Theme, width: u16) -> Line<'static> {
    let chip = toggle(width, panel.markdown_toggle_available());
    if panel.tabs().is_empty() {
        return Line::styled("no file open", Style::default().fg(theme.muted));
    }
    let strip_width = panel.tab_strip_width(width);
    let strip = tabs::strip(panel.tabs(), panel.tab_scroll(), strip_width);
    let overflow = Style::default().fg(theme.accent);
    let mut spans = Vec::new();
    if strip.left {
        spans.push(Span::styled(tabs::OVERFLOW_LEFT.to_string(), overflow));
    }
    for (i, _) in &strip.cells {
        let tab = &panel.tabs()[*i];
        let mut style = if *i == panel.active_tab() {
            Style::default()
                .fg(theme.selected_fg)
                .bg(theme.selected_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.muted)
        };
        if tab.status.is_preview() {
            style = style.add_modifier(Modifier::ITALIC);
        }
        spans.push(Span::styled(tabs::label(&tab.path), style));
    }
    if strip.right {
        let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        spans.push(Span::raw(
            " ".repeat((strip_width as usize).saturating_sub(used + 1)),
        ));
        spans.push(Span::styled(tabs::OVERFLOW_RIGHT.to_string(), overflow));
    }
    let Some(chip) = chip else {
        return Line::from(spans);
    };
    // タブはチップの桁へはみ出せない。1 枚も入らない幅では帯が溢れて描かれるので、
    // 切ってから並べないとチップが画面の外へ押し出される。
    let room = chip.raw.start as usize;
    let pieces = spans
        .into_iter()
        .map(|s| (s.style, s.content.into_owned()))
        .collect();
    let mut spans = clip(pieces, 0, room);
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    spans.push(Span::raw(" ".repeat(room - used)));
    spans.extend(toggle_spans(panel.is_showing_rendered_markdown(), theme));
    Line::from(spans)
}

/// 本文。素の表示と diff で行の組み方が違うだけで、窓の切り方は同じ。
pub fn body(
    panel: &ViewerPanel,
    review: &ReviewState,
    theme: &Theme,
    icons: IconSet,
    width: u16,
    height: usize,
) -> Vec<Line<'static>> {
    if let Some(error) = &panel.content.error {
        return vec![Line::styled(
            format!("  \u{26a0} {error}"),
            Style::default().fg(theme.error),
        )];
    }
    if panel.content.path.is_none() {
        return vec![Line::styled(
            "  select a file in the explorer",
            Style::default().fg(theme.muted),
        )];
    }
    if let Some(preview) = &panel.content.media {
        return media_body(preview, theme, height);
    }
    if panel.is_showing_rendered_markdown() {
        return panel
            .content
            .rendered
            .iter()
            .skip(panel.scroll.md)
            .take(height)
            .cloned()
            .collect();
    }
    let comments = panel
        .content
        .path
        .as_deref()
        .map_or_else(Vec::new, |path| review.for_file(path));
    let mut lines = summary_banner(panel, review, theme, width as usize);
    let rest = height.saturating_sub(lines.len());
    lines.extend(if panel.diff.active {
        diff_body(panel, review, &comments, theme, icons, width, rest)
    } else {
        file_body(panel, review, &comments, theme, icons, width, rest)
    });
    lines
}

/// 絵と、その下の寸法・ファイルサイズの行。
fn media_body(preview: &super::media::Preview, theme: &Theme, height: usize) -> Vec<Line<'static>> {
    use super::media::Preview;
    match preview {
        Preview::Loading => vec![Line::styled(
            "  rendering the image\u{2026}",
            Style::default().fg(theme.muted),
        )],
        Preview::Failed(reason) => vec![Line::styled(
            format!("  \u{26a0} {reason}"),
            Style::default().fg(theme.error),
        )],
        Preview::Ready(rendered) => {
            let mut lines: Vec<Line<'static>> = rendered
                .lines
                .iter()
                .take(height.saturating_sub(1))
                .cloned()
                .collect();
            lines.push(Line::styled(
                super::media::caption(rendered),
                Style::default().fg(theme.muted),
            ));
            lines
        }
    }
}

/// このブランチの変更サマリ。差分のときだけ出す — 素のファイルを開くたびに
/// 本文の頭を数行奪わない。
fn summary_banner(
    panel: &ViewerPanel,
    review: &ReviewState,
    theme: &Theme,
    width: usize,
) -> Vec<Line<'static>> {
    if !panel.diff.active {
        return Vec::new();
    }
    let Some(summary) = review.summary() else {
        return Vec::new();
    };
    let heading = " \u{25a3} branch summary";
    let mut lines = vec![Line::styled(
        format!(
            "{heading}{}",
            " ".repeat(width.saturating_sub(heading.chars().count()))
        ),
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    )];
    let mut body = summary.lines();
    lines.extend(
        body.by_ref()
            .take(SUMMARY_ROWS)
            .map(|line| Line::styled(format!(" {line}"), Style::default().fg(theme.fg))),
    );
    if body.next().is_some() {
        lines.push(Line::styled(
            " \u{22ef} more in the comment summary",
            Style::default().fg(theme.hint),
        ));
    }
    lines.push(Line::styled(
        "\u{2500}".repeat(width),
        Style::default().fg(theme.border_secondary),
    ));
    lines
}

fn file_body(
    panel: &ViewerPanel,
    review: &ReviewState,
    comments: &[&ReviewComment],
    theme: &Theme,
    icons: IconSet,
    width: u16,
    height: usize,
) -> Vec<Line<'static>> {
    let total = panel.content.lines.len();
    let digits = digit_count(total);
    let mark = if comments.is_empty() { 0 } else { MARK };
    let badge = badge_width(panel);
    let text_width = (width as usize).saturating_sub(digits + GUTTER_FIXED + mark + badge);

    let mut out = Vec::with_capacity(height);
    for line_1 in panel.fold.visible_from(panel.scroll.line + 1, total) {
        if out.len() >= height {
            break;
        }
        let mut spans = Vec::new();
        if mark > 0 {
            spans.push(thread::marker(comments, line_1, theme, icons));
        }
        spans.extend(gutter_spans(panel, theme, icons, line_1, digits, badge));
        match panel.nav.labels.as_ref().filter(|l| l.line == line_1) {
            Some(labels) => spans.extend(
                super::code_nav::label_line(
                    labels,
                    &panel.content.lines[line_1 - 1],
                    panel.scroll.column,
                    text_width,
                    theme,
                )
                .spans,
            ),
            None => spans.extend(highlighted_spans(
                panel,
                theme,
                line_1,
                panel.scroll.column,
                text_width,
            )),
        }
        if let Some(hidden) = panel.fold.hidden_count(line_1) {
            spans.push(Span::styled(
                format!("  \u{22ef} {hidden} lines"),
                Style::default().fg(theme.hint),
            ));
        }
        let line = Line::from(spans);
        out.push(if panel.selection.contains(line_1) {
            line.style(
                Style::default()
                    .bg(theme.line_selected_bg)
                    .fg(theme.line_selected_fg),
            )
        } else {
            line
        });
        out.extend(thread_rows(
            panel, review, comments, line_1, theme, icons, width,
        ));
    }
    out.truncate(height);
    out
}

/// その行に開いているスレッド。閉じていれば空。
fn thread_rows(
    panel: &ViewerPanel,
    review: &ReviewState,
    comments: &[&ReviewComment],
    line_1: usize,
    theme: &Theme,
    icons: IconSet,
    width: u16,
) -> Vec<Line<'static>> {
    if !panel.threads.is_open(comments, line_1) {
        return Vec::new();
    }
    thread::lines(
        review,
        comments,
        line_1,
        theme,
        icons,
        width as usize,
        MARK + 2,
    )
}

/// 行番号、折りたたみマーカー、テスト実行ボタン、仕切り。
fn gutter_spans(
    panel: &ViewerPanel,
    theme: &Theme,
    icons: IconSet,
    line_1: usize,
    digits: usize,
    badge: usize,
) -> Vec<Span<'static>> {
    let marker = if panel.fold.is_collapsed(line_1) {
        '\u{25b8}'
    } else if panel.fold.is_foldable(line_1) {
        '\u{25be}'
    } else {
        ' '
    };
    let muted = Style::default().fg(theme.muted);
    let mut spans = vec![Span::styled(format!("{line_1:>digits$}{marker}"), muted)];
    if badge > 0 {
        spans.push(match panel.content.tests.contains_key(&line_1) {
            true => Span::styled(
                conductor_core::icons::RUN_TEST.labeled(icons),
                Style::default()
                    .fg(theme.success)
                    .add_modifier(Modifier::BOLD),
            ),
            false => Span::raw(" ".repeat(badge)),
        });
    }
    spans.push(Span::styled(" \u{2502} ", muted));
    spans
}

/// ハイライト済みの断片から、横スクロールぶんを飛ばして幅に収める。
fn highlighted_spans(
    panel: &ViewerPanel,
    theme: &Theme,
    line_1: usize,
    skip: usize,
    width: usize,
) -> Vec<Span<'static>> {
    let Some(raw) = panel.content.lines.get(line_1 - 1) else {
        return Vec::new();
    };
    let matched = panel.search.is_match(line_1 - 1);
    let plain = || {
        vec![(
            if matched {
                Style::default()
                    .fg(theme.search_match_fg)
                    .bg(theme.search_match_bg)
            } else {
                Style::default().fg(theme.fg)
            },
            raw.clone(),
        )]
    };
    let pieces = panel
        .content
        .highlighted
        .get(line_1 - 1)
        .cloned()
        .filter(|_| !matched)
        .unwrap_or_else(plain);
    clip(pieces, skip, width)
}

/// 文字数で数えて skip 列を捨て、width 列ぶんだけ残す。
fn clip(pieces: Vec<(Style, String)>, skip: usize, width: usize) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut col = 0usize;
    let mut used = 0usize;
    for (style, text) in pieces {
        if used >= width {
            break;
        }
        let len = text.chars().count();
        if col + len <= skip {
            col += len;
            continue;
        }
        let start = skip.saturating_sub(col);
        let take = (len - start).min(width - used);
        let slice: String = text.chars().skip(start).take(take).collect();
        col += len;
        used += take;
        if !slice.is_empty() {
            out.push(Span::styled(slice, style));
        }
    }
    out
}

fn diff_body(
    panel: &ViewerPanel,
    review: &ReviewState,
    comments: &[&ReviewComment],
    theme: &Theme,
    icons: IconSet,
    width: u16,
    height: usize,
) -> Vec<Line<'static>> {
    let digits = digit_count(panel.diff.max_line_no);
    let mark = if comments.is_empty() { 0 } else { MARK };
    let mut out = Vec::with_capacity(height);
    let rows: Vec<(Line<'static>, Option<usize>)> = if panel.diff.side_by_side {
        side_by_side(&panel.diff.entries)
            .iter()
            .skip(panel.scroll.diff)
            .map(|row| (side_line(row, theme, digits, width, panel.scroll), None))
            .collect()
    } else {
        panel
            .diff
            .entries
            .iter()
            .skip(panel.scroll.diff)
            .map(|entry| {
                let mut spans = Vec::new();
                if mark > 0 {
                    let line_1 = entry.new_line_no().unwrap_or(0);
                    spans.push(thread::marker(comments, line_1, theme, icons));
                }
                let body = unified_line(
                    entry,
                    theme,
                    digits,
                    (width as usize).saturating_sub(mark),
                    panel.scroll.column,
                );
                let style = body.style;
                spans.extend(body.spans);
                (Line::from(spans).style(style), entry.new_line_no())
            })
            .collect()
    };
    for (line, at) in rows {
        if out.len() >= height {
            break;
        }
        out.push(line);
        if let Some(line_1) = at {
            out.extend(thread_rows(
                panel, review, comments, line_1, theme, icons, width,
            ));
        }
    }
    out.truncate(height);
    out
}

fn unified_line(
    entry: &Entry,
    theme: &Theme,
    digits: usize,
    width: usize,
    skip: usize,
) -> Line<'static> {
    match entry {
        Entry::HunkSeparator { func_header } => separator(theme, func_header.as_deref(), width),
        Entry::ExpandableContext {
            hidden_count,
            func_header,
            ..
        } => {
            let header = func_header.as_deref().unwrap_or("");
            separator(
                theme,
                Some(&format!("\u{22ef} {hidden_count} lines hidden {header}")),
                width,
            )
        }
        Entry::Line {
            tag,
            new_line_no,
            content,
            inline_segments,
            ..
        } => {
            let (sign, fg, bg) = match tag {
                DiffLineTag::Insert => ('+', theme.diff_add, Some(theme.diff_add_bg)),
                DiffLineTag::Delete => ('-', theme.diff_del, Some(theme.diff_del_bg)),
                DiffLineTag::Equal => (' ', theme.fg, None),
            };
            let no = new_line_no.map_or_else(|| " ".repeat(digits), |n| format!("{n:>digits$}"));
            let mut spans = vec![Span::styled(
                format!("{sign} {no} \u{2502} "),
                Style::default().fg(theme.muted),
            )];
            let text_width = width.saturating_sub(digits + DIFF_SIGN + GUTTER_FIXED);
            spans.extend(clip(
                inline_pieces(content, inline_segments, tag, theme, fg),
                skip,
                text_width,
            ));
            match bg {
                Some(bg) => Line::from(spans).style(Style::default().bg(bg)),
                None => Line::from(spans),
            }
        }
    }
}

/// 行内で実際に変わった箇所だけ背景を濃くする。単語 diff が無ければ 1 断片。
fn inline_pieces(
    content: &str,
    segments: &[conductor_core::diff_state::InlineSegment],
    tag: &DiffLineTag,
    theme: &Theme,
    fg: ratatui::style::Color,
) -> Vec<(Style, String)> {
    if segments.is_empty() {
        return vec![(Style::default().fg(fg), content.to_string())];
    }
    let emphasis = match tag {
        DiffLineTag::Insert => theme.diff_add_bg_emphasis,
        _ => theme.diff_del_bg_emphasis,
    };
    segments
        .iter()
        .map(|segment| {
            let style = Style::default().fg(fg);
            let style = if segment.emphasized {
                style.bg(emphasis).add_modifier(Modifier::BOLD)
            } else {
                style
            };
            (style, segment.text.clone())
        })
        .collect()
}

fn separator(theme: &Theme, label: Option<&str>, width: usize) -> Line<'static> {
    let style = Style::default().fg(theme.diff_section_header);
    let Some(label) = label else {
        return Line::styled("\u{2500}".repeat(width), style);
    };
    let text = format!(" {label} ");
    let fill = width.saturating_sub(text.chars().count() + 2);
    Line::from(vec![
        Span::styled("\u{2500}\u{2500}", style),
        Span::styled(text, style.add_modifier(Modifier::BOLD)),
        Span::styled("\u{2500}".repeat(fill), style),
    ])
}

/// 左右 2 列の 1 行。区切りは幅いっぱいに伸ばす。
fn side_line(
    row: &SideRow<'_>,
    theme: &Theme,
    digits: usize,
    width: u16,
    scroll: Scroll,
) -> Line<'static> {
    let width = width as usize;
    match row {
        SideRow::Span(entry) => unified_line(entry, theme, digits, width, scroll.column),
        SideRow::Split { left, right } => {
            let half = width / 2;
            let mut spans = half_line(*left, theme, digits, half, scroll.column, true);
            spans.push(Span::styled(
                "\u{2502}",
                Style::default().fg(theme.border_secondary),
            ));
            spans.extend(half_line(
                *right,
                theme,
                digits,
                width - half - 1,
                scroll.column,
                false,
            ));
            Line::from(spans)
        }
    }
}

fn half_line(
    entry: Option<&Entry>,
    theme: &Theme,
    digits: usize,
    width: usize,
    skip: usize,
    old_side: bool,
) -> Vec<Span<'static>> {
    let Some(Entry::Line {
        tag,
        old_line_no,
        new_line_no,
        content,
        inline_segments,
    }) = entry
    else {
        return vec![Span::raw(" ".repeat(width))];
    };
    let line_no = if old_side { old_line_no } else { new_line_no };
    let fg = match tag {
        DiffLineTag::Insert => theme.diff_add,
        DiffLineTag::Delete => theme.diff_del,
        DiffLineTag::Equal => theme.fg,
    };
    let no = line_no.map_or_else(|| " ".repeat(digits), |n| format!("{n:>digits$}"));
    let mut spans = vec![Span::styled(
        format!("{no} "),
        Style::default().fg(theme.muted),
    )];
    let text_width = width.saturating_sub(digits + 1);
    let pieces = inline_pieces(content, inline_segments, tag, theme, fg);
    let body = clip(pieces, skip, text_width);
    let used: usize = body.iter().map(|s| s.content.chars().count()).sum();
    spans.extend(body);
    spans.push(Span::raw(" ".repeat(text_width.saturating_sub(used))));
    spans
}

pub fn digit_count(n: usize) -> usize {
    n.max(1).ilog10() as usize + 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use conductor_core::config::Config;
    use conductor_core::diff_state::{DiffHunk, DiffLine, FileDiff, InlineSegment};

    fn panel(lines: &[&str]) -> ViewerPanel {
        let mut panel = ViewerPanel::new(&Config::default());
        panel.content.path = Some("a.rs".into());
        panel.content.lines = lines.iter().map(|s| s.to_string()).collect();
        panel
    }

    fn texts(lines: &[Line<'_>]) -> Vec<String> {
        lines.iter().map(|l| l.to_string()).collect()
    }

    #[test]
    fn 本文は行番号と仕切りを付けて窓のぶんだけ出す() {
        let panel = panel(&["one", "two", "three", "four"]);
        let lines = body(
            &panel,
            &ReviewState::default(),
            &Theme::default(),
            IconSet::Unicode,
            40,
            2,
        );
        assert_eq!(texts(&lines), ["1  \u{2502} one", "2  \u{2502} two"]);
    }

    #[test]
    fn 横スクロールは本文だけを削りガターは残す() {
        let mut panel = panel(&["abcdefgh"]);
        panel.scroll.column = 4;
        let lines = body(
            &panel,
            &ReviewState::default(),
            &Theme::default(),
            IconSet::Unicode,
            40,
            1,
        );
        assert_eq!(texts(&lines), ["1  \u{2502} efgh"]);
    }

    #[test]
    fn 未選択と読み込み失敗は別の行になる() {
        let empty = ViewerPanel::new(&Config::default());
        assert!(
            texts(&body(
                &empty,
                &ReviewState::default(),
                &Theme::default(),
                IconSet::Unicode,
                40,
                5
            ))[0]
                .contains("select a file"),
            "未選択"
        );

        let mut failed = panel(&[]);
        failed.content.error = Some("binary file".into());
        assert!(
            texts(&body(
                &failed,
                &ReviewState::default(),
                &Theme::default(),
                IconSet::Unicode,
                40,
                5
            ))[0]
                .contains("binary file")
        );
    }

    #[test]
    fn 畳んだ行は本文から消え見出しに印が付く() {
        let source = "fn a() {\n    b();\n    c();\n}\n";
        let mut panel = panel(&["fn a() {", "    b();", "    c();", "}"]);
        panel
            .fold
            .install(super::super::fold::compute(source, "a.rs"), "a.rs");
        panel.fold.close(1);
        let lines = texts(&body(
            &panel,
            &ReviewState::default(),
            &Theme::default(),
            IconSet::Unicode,
            40,
            10,
        ));
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains('\u{25b8}'), "{:?}", lines[0]);
    }

    #[test]
    fn diffは符号と行番号を並べ区切りに隠れた行数を出す() {
        let mut panel = panel(&["a", "b", "c"]);
        let file_diff = FileDiff {
            path: "a.rs".into(),
            added_lines: 1,
            deleted_lines: 1,
            hunks: vec![DiffHunk {
                lines: vec![
                    DiffLine {
                        tag: DiffLineTag::Delete,
                        old_line_no: Some(2),
                        new_line_no: None,
                        inline_segments: Vec::new(),
                        content: "old".into(),
                    },
                    DiffLine {
                        tag: DiffLineTag::Insert,
                        old_line_no: None,
                        new_line_no: Some(2),
                        inline_segments: vec![
                            InlineSegment {
                                text: "ne".into(),
                                emphasized: true,
                            },
                            InlineSegment {
                                text: "w".into(),
                                emphasized: false,
                            },
                        ],
                        content: "new".into(),
                    },
                ],
                func_header: None,
            }],
        };
        panel.diff.build(&file_diff, 3);

        let lines = texts(&body(
            &panel,
            &ReviewState::default(),
            &Theme::default(),
            IconSet::Unicode,
            40,
            10,
        ));
        assert!(lines[0].contains("1 lines hidden"), "{:?}", lines[0]);
        assert!(lines[1].starts_with("-   \u{2502} old"), "{:?}", lines[1]);
        assert!(lines[2].starts_with("+ 2 \u{2502} new"), "{:?}", lines[2]);
        assert!(lines[3].contains("1 lines hidden"), "{:?}", lines[3]);

        panel.diff.side_by_side = true;
        let wide = texts(&body(
            &panel,
            &ReviewState::default(),
            &Theme::default(),
            IconSet::Unicode,
            40,
            10,
        ));
        assert!(
            wide[1].contains("old") && wide[1].contains("new"),
            "{:?}",
            wide[1]
        );
    }

    fn review_with(comments: Vec<conductor_core::review_store::ReviewComment>) -> ReviewState {
        let mut review = ReviewState::default();
        review.install(Ok(crate::review::Snapshot {
            branch: "main".into(),
            comments,
            ..crate::review::Snapshot::default()
        }));
        review
    }

    fn render_body(panel: &ViewerPanel, review: &ReviewState, height: usize) -> Vec<String> {
        texts(&body(
            panel,
            review,
            &Theme::default(),
            IconSet::Unicode,
            40,
            height,
        ))
    }

    /// 印の桁はコメントのあるファイルでだけ開ける。全ファイルで 2 桁譲ると、
    /// レビューしていない読み物のときに理由の無い余白になる。
    #[test]
    fn コメントのある行だけが印の桁を開く() {
        let panel = panel(&["one", "two", "three"]);
        assert_eq!(
            render_body(&panel, &ReviewState::default(), 1),
            ["1  \u{2502} one"]
        );

        let review = review_with(vec![crate::review::tests::comment("a", "a.rs", 3, None)]);
        let lines = render_body(&panel, &review, 2);
        assert!(lines[0].starts_with("  1"), "{:?}", lines[0]);
        assert!(lines[1].starts_with("  2"), "{:?}", lines[1]);
    }

    #[test]
    fn 開いたスレッドは行の直後に割り込み窓の高さを食う() {
        let panel = panel(&["one", "two", "three"]);
        let review = review_with(vec![crate::review::tests::comment("a", "a.rs", 1, None)]);
        let lines = render_body(&panel, &review, 4);
        assert!(lines[0].contains("one"));
        assert!(
            lines.iter().skip(1).any(|l| l.contains("body of a")),
            "{lines:?}"
        );
        assert_eq!(lines.len(), 4, "窓の高さを超えない");
    }

    #[test]
    fn 変更サマリは差分のときだけ本文の上に出る() {
        let mut panel = panel(&["a"]);
        let mut review = ReviewState::default();
        review.install(Ok(crate::review::Snapshot {
            summary: Some("rewrote the parser".into()),
            ..crate::review::Snapshot::default()
        }));
        assert!(
            !render_body(&panel, &review, 10)
                .iter()
                .any(|l| l.contains("rewrote")),
        );

        panel.diff.build(
            &FileDiff {
                path: "a.rs".into(),
                added_lines: 1,
                deleted_lines: 0,
                hunks: vec![DiffHunk {
                    lines: vec![DiffLine {
                        tag: DiffLineTag::Insert,
                        old_line_no: None,
                        new_line_no: Some(1),
                        inline_segments: Vec::new(),
                        content: "a".into(),
                    }],
                    func_header: None,
                }],
            },
            1,
        );
        let lines = render_body(&panel, &review, 10);
        assert!(lines[0].contains("branch summary"), "{:?}", lines[0]);
        assert!(lines[1].contains("rewrote the parser"), "{:?}", lines[1]);
    }

    #[test]
    fn タブ帯はアクティブを太字にしパスの末尾を残す() {
        let mut panel = panel(&["x"]);
        panel.activate_tab_for(
            "src/very/deep/directory/name.rs",
            super::super::tabs::TabStatus::Persistent,
        );
        let row = tab_row(&panel, &Theme::default(), 40);
        assert!(row.to_string().contains("name.rs"));
        assert!(row.to_string().contains('\u{2026}'), "{row}");

        let empty = ViewerPanel::new(&Config::default());
        assert_eq!(
            tab_row(&empty, &Theme::default(), 40).to_string(),
            "no file open"
        );
    }

    /// 桁はテストのあるファイルでだけ開ける。全ファイルで 2 桁譲ると、テストの無い
    /// ファイルを読むときに理由の無い余白になる。
    #[test]
    fn 実行できる行だけが実行ボタンの桁を開く() {
        let plain = panel(&["fn a() {}", "fn b() {}"]);
        assert_eq!(
            texts(&body(
                &plain,
                &ReviewState::default(),
                &Theme::default(),
                IconSet::Unicode,
                40,
                1
            )),
            ["1  \u{2502} fn a() {}"]
        );

        let mut with_tests = panel(&["fn a() {}", "fn b() {}"]);
        with_tests.content.tests.insert(
            2,
            conductor_core::test_run::TestRun {
                kind: conductor_core::test_run::TestRunKind::Func,
                label: "b".into(),
                command: "cargo test b".into(),
            },
        );
        let lines = texts(&body(
            &with_tests,
            &ReviewState::default(),
            &Theme::default(),
            IconSet::Unicode,
            40,
            2,
        ));
        assert_eq!(lines[0], "1    \u{2502} fn a() {}", "桁は開くが印は無い");
        assert!(lines[1].starts_with("2 \u{25b8}"), "{:?}", lines[1]);
    }

    /// 描いた列と当たり判定が同じ 1 つの計算を読む。ずれると出ていないチップが
    /// 押せたり、押しても切り替わらない列ができる。
    #[test]
    fn トグルは右端に出て描いた列と当たり判定が一致する() {
        let mut panel = panel(&["# Title"]);
        panel.activate_tab_for("notes.md", super::super::tabs::TabStatus::Persistent);
        panel.content.path = Some("notes.md".into());

        for width in [MIN_TAB_ROW, MIN_TAB_ROW + 1, 40, 120] {
            let chip = toggle(width, true).expect("MIN_TAB_ROW 以上なら出る");
            let drawn: Vec<char> = tab_row(&panel, &Theme::default(), width)
                .to_string()
                .chars()
                .collect();
            let at = |x: u16| drawn[x as usize];
            assert_eq!(at(chip.raw.start), '[', "w={width}");
            assert_eq!(at(chip.rendered.start), '|', "w={width}");
            assert_eq!(at(chip.rendered.end - 1), ']', "w={width}");
            assert_eq!(chip.rendered.end, width, "右端まで使う");
            assert_eq!(drawn.len(), width as usize, "行はちょうど幅ぶん");
        }

        assert!(toggle(MIN_TAB_ROW - 1, true).is_none(), "狭ければ出さない");
        assert!(toggle(80, false).is_none(), "markdown でなければ出さない");
    }

    #[test]
    fn 桁数は行数で決まる() {
        for (n, expected) in [(0, 1), (1, 1), (9, 1), (10, 2), (999, 3), (1000, 4)] {
            assert_eq!(digit_count(n), expected, "{n}");
        }
    }
}
