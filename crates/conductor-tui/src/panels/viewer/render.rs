//! Viewer の描画。行は [body] が組み、frame へ渡すのは最後の 1 手だけ。
//!
//! 中身の判断を [ratatui::Frame] の外に出しているので、テストは Line の並びを見る。

use conductor_core::diff_state::DiffLineTag;
use conductor_core::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};

use super::diff::{Entry, SideRow, side_by_side};
use super::{Scroll, ViewerPanel};
use crate::workspace::Workspace;

/// ガターのうち行番号が使わない列: 折りたたみマーカー(1) + 空白(1) + '│'(1) + 空白(1)。
/// diff 表示ではさらに +/- の 1 列と空白 1 列。
const GUTTER_FIXED: usize = 4;
const DIFF_SIGN: usize = 2;

/// 本文の上に載るタブ帯の高さ。[super::ViewerPanel::sync_layout] も同じ値を引く。
pub const TAB_ROW: u16 = 1;

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
    let lines = body(panel, &ws.theme, body_area.width, body_area.height as usize);
    frame.render_widget(Paragraph::new(lines), body_area);
    scrollbar(frame, body_area, panel);
}

/// 畳んだぶんを除いた尺で出す。畳んだまま端まで送ったのにつまみが半分、が起きない。
fn scrollbar(frame: &mut Frame, area: Rect, panel: &ViewerPanel) {
    let (total, at) = if panel.diff.active {
        (panel.diff.entries.len(), panel.scroll.diff)
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
    if panel.tabs().is_empty() {
        return Line::styled("no file open", Style::default().fg(theme.muted));
    }
    let mut spans = Vec::new();
    let mut used = 0usize;
    for (i, tab) in panel.tabs().iter().enumerate() {
        let label = format!(" {} ", elide_head(&tab.path, 28));
        if used + label.chars().count() > width as usize && !spans.is_empty() {
            spans.push(Span::styled(">", Style::default().fg(theme.muted)));
            break;
        }
        used += label.chars().count();
        let mut style = if i == panel.active_tab() {
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
        spans.push(Span::styled(label, style));
    }
    Line::from(spans)
}

/// 本文。素の表示と diff で行の組み方が違うだけで、窓の切り方は同じ。
pub fn body(panel: &ViewerPanel, theme: &Theme, width: u16, height: usize) -> Vec<Line<'static>> {
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
    if panel.diff.active {
        diff_body(panel, theme, width, height)
    } else {
        file_body(panel, theme, width, height)
    }
}

fn file_body(panel: &ViewerPanel, theme: &Theme, width: u16, height: usize) -> Vec<Line<'static>> {
    let total = panel.content.lines.len();
    let digits = digit_count(total);
    let gutter = digits + GUTTER_FIXED;
    let text_width = (width as usize).saturating_sub(gutter);

    panel
        .fold
        .visible_from(panel.scroll.line + 1, total)
        .take(height)
        .map(|line_1| {
            let mut spans = gutter_spans(panel, theme, line_1, digits);
            spans.extend(highlighted_spans(
                panel,
                theme,
                line_1,
                panel.scroll.column,
                text_width,
            ));
            if let Some(hidden) = panel.fold.hidden_count(line_1) {
                spans.push(Span::styled(
                    format!("  \u{22ef} {hidden} lines"),
                    Style::default().fg(theme.hint),
                ));
            }
            let selected = panel.selection.contains(line_1);
            let line = Line::from(spans);
            if selected {
                line.style(
                    Style::default()
                        .bg(theme.line_selected_bg)
                        .fg(theme.line_selected_fg),
                )
            } else {
                line
            }
        })
        .collect()
}

/// 行番号と折りたたみマーカーと仕切り。
fn gutter_spans(
    panel: &ViewerPanel,
    theme: &Theme,
    line_1: usize,
    digits: usize,
) -> Vec<Span<'static>> {
    let marker = if panel.fold.is_collapsed(line_1) {
        '\u{25b8}'
    } else if panel.fold.is_foldable(line_1) {
        '\u{25be}'
    } else {
        ' '
    };
    vec![Span::styled(
        format!("{line_1:>digits$}{marker} \u{2502} "),
        Style::default().fg(theme.muted),
    )]
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

fn diff_body(panel: &ViewerPanel, theme: &Theme, width: u16, height: usize) -> Vec<Line<'static>> {
    let digits = digit_count(panel.diff.max_line_no);
    if panel.diff.side_by_side {
        let rows = side_by_side(&panel.diff.entries);
        return rows
            .iter()
            .skip(panel.scroll.diff)
            .take(height)
            .map(|row| side_line(row, theme, digits, width, panel.scroll))
            .collect();
    }
    panel
        .diff
        .entries
        .iter()
        .skip(panel.scroll.diff)
        .take(height)
        .map(|entry| unified_line(entry, theme, digits, width as usize, panel.scroll.column))
        .collect()
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

/// 先頭を省いてファイル名を残す。
fn elide_head(path: &str, budget: usize) -> String {
    let len = path.chars().count();
    if len <= budget {
        return path.to_string();
    }
    let kept: String = path.chars().skip(len - budget + 1).collect();
    format!("\u{2026}{kept}")
}

fn digit_count(n: usize) -> usize {
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
        let lines = body(&panel, &Theme::default(), 40, 2);
        assert_eq!(texts(&lines), ["1  \u{2502} one", "2  \u{2502} two"]);
    }

    #[test]
    fn 横スクロールは本文だけを削りガターは残す() {
        let mut panel = panel(&["abcdefgh"]);
        panel.scroll.column = 4;
        let lines = body(&panel, &Theme::default(), 40, 1);
        assert_eq!(texts(&lines), ["1  \u{2502} efgh"]);
    }

    #[test]
    fn 未選択と読み込み失敗は別の行になる() {
        let empty = ViewerPanel::new(&Config::default());
        assert!(
            texts(&body(&empty, &Theme::default(), 40, 5))[0].contains("select a file"),
            "未選択"
        );

        let mut failed = panel(&[]);
        failed.content.error = Some("binary file".into());
        assert!(texts(&body(&failed, &Theme::default(), 40, 5))[0].contains("binary file"));
    }

    #[test]
    fn 畳んだ行は本文から消え見出しに印が付く() {
        let source = "fn a() {\n    b();\n    c();\n}\n";
        let mut panel = panel(&["fn a() {", "    b();", "    c();", "}"]);
        panel
            .fold
            .install(super::super::fold::compute(source, "a.rs"), "a.rs");
        panel.fold.close(1);
        let lines = texts(&body(&panel, &Theme::default(), 40, 10));
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

        let lines = texts(&body(&panel, &Theme::default(), 40, 10));
        assert!(lines[0].contains("1 lines hidden"), "{:?}", lines[0]);
        assert!(lines[1].starts_with("-   \u{2502} old"), "{:?}", lines[1]);
        assert!(lines[2].starts_with("+ 2 \u{2502} new"), "{:?}", lines[2]);
        assert!(lines[3].contains("1 lines hidden"), "{:?}", lines[3]);

        panel.diff.side_by_side = true;
        let wide = texts(&body(&panel, &Theme::default(), 40, 10));
        assert!(
            wide[1].contains("old") && wide[1].contains("new"),
            "{:?}",
            wide[1]
        );
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

    #[test]
    fn 桁数は行数で決まる() {
        for (n, expected) in [(0, 1), (1, 1), (9, 1), (10, 2), (999, 3), (1000, 4)] {
            assert_eq!(digit_count(n), expected, "{n}");
        }
    }
}
