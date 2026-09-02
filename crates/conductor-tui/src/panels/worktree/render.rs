//! 全幅のストリップと、フォーカス中に開く一覧。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use conductor_core::git_engine::WorktreeInfo;

use crate::strip::visible_window;
use crate::workspace::Workspace;

/// worktree 1 つを一目で表す文字列。ブランチ、変更数、ahead/behind。
fn chip_text(worktree: &WorktreeInfo, waiting: bool, active: bool) -> String {
    let mut text = String::from(" ");
    if waiting {
        text.push_str("\u{23f3} ");
    } else if active {
        text.push_str("\u{25cf} ");
    }
    text.push_str(&worktree.branch);
    if !worktree.is_clean {
        text.push_str(&format!(
            " ~{}",
            worktree.added + worktree.modified + worktree.deleted
        ));
    }
    if let Some(ahead) = worktree.ahead.filter(|a| *a > 0) {
        text.push_str(&format!(" \u{2191}{ahead}"));
    }
    if let Some(behind) = worktree.behind.filter(|b| *b > 0) {
        text.push_str(&format!(" \u{2193}{behind}"));
    }
    text.push(' ');
    text
}

fn width(text: &str) -> u16 {
    UnicodeWidthStr::width(text) as u16
}

pub fn strip(ws: &Workspace, area: Rect) -> Line<'static> {
    let theme = &ws.theme;
    let panel = &ws.panels.worktree;
    let selected = panel.selected_index();

    let lead = if panel.is_busy() {
        Span::styled(
            "\u{22ef} ",
            Style::default()
                .fg(theme.success)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            "\u{2387} ",
            Style::default()
                .fg(theme.muted)
                .add_modifier(Modifier::BOLD),
        )
    };
    let mut spans = vec![lead];
    if panel.list().is_empty() {
        spans.push(Span::styled(
            "no worktrees",
            Style::default().fg(theme.muted),
        ));
        return Line::from(spans);
    }

    let chips: Vec<(String, &'static str)> = panel
        .list()
        .iter()
        .map(|worktree| {
            let text = chip_text(
                worktree,
                ws.panels.terminal.is_waiting(&worktree.path),
                ws.panels.terminal.is_active(&worktree.path),
            );
            (text, if worktree.is_main { "" } else { "[x]" })
        })
        .collect();

    // 窓は毎フレーム選択から決め直す。描画はスクロール位置を書き戻せないし、
    // 選択が必ず見えていれば覚えておく必要もない。
    let slots: Vec<u16> = chips.iter().map(|(t, d)| width(t) + width(d)).collect();
    let sep = "\u{2502} ";
    let avail = area.width.saturating_sub(width("\u{2387}  [+]"));
    let (start, end) = visible_window(&slots, width(sep), avail, 0, selected, true);

    if start > 0 {
        spans.push(Span::styled(
            format!("\u{2039}{start} "),
            Style::default().fg(theme.muted),
        ));
    }
    for (i, (text, delete)) in chips.iter().enumerate().take(end).skip(start) {
        if i > start {
            spans.push(Span::styled(
                sep,
                Style::default().fg(theme.border_secondary),
            ));
        }
        let style = if i == selected {
            Style::default()
                .fg(theme.selected_fg)
                .bg(theme.selected_bg)
                .add_modifier(Modifier::BOLD)
        } else if ws.panels.terminal.is_waiting(&panel.list()[i].path) {
            Style::default().fg(theme.warning)
        } else {
            Style::default().fg(theme.muted)
        };
        spans.push(Span::styled(text.clone(), style));
        if !delete.is_empty() {
            spans.push(Span::styled(*delete, Style::default().fg(theme.error)));
        }
    }
    if end < chips.len() {
        spans.push(Span::styled(
            format!(" {}\u{203a}", chips.len() - end),
            Style::default().fg(theme.muted),
        ));
    }
    spans.push(Span::styled(
        " [+]",
        Style::default()
            .fg(theme.success)
            .add_modifier(Modifier::BOLD),
    ));
    Line::from(spans)
}

/// フォーカス中だけ画面の中央に開く一覧。ストリップは幅の都合で削るので、
/// 全部を並べて見る場所を別に持つ。
pub fn list(frame: &mut Frame, area: Rect, ws: &Workspace) {
    let theme = &ws.theme;
    let panel = &ws.panels.worktree;
    let selected = panel.selected_index();

    let rows: Vec<Line> = panel
        .list()
        .iter()
        .enumerate()
        .map(|(i, worktree)| {
            let marker = if i == selected { "\u{25b8} " } else { "  " };
            let style = if i == selected {
                Style::default()
                    .fg(theme.selected_fg)
                    .bg(theme.selected_bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };
            let mut spans = vec![Span::styled(format!("{marker}{}", worktree.branch), style)];
            if !worktree.is_clean {
                spans.push(Span::styled(
                    format!(
                        " ~{}",
                        worktree.added + worktree.modified + worktree.deleted
                    ),
                    Style::default().fg(theme.warning),
                ));
            }
            spans.push(Span::styled(
                format!("  {}", worktree.path.display()),
                Style::default().fg(theme.dir_fg),
            ));
            Line::from(spans)
        })
        .collect();

    let rows = if rows.is_empty() {
        vec![Line::styled(
            "  no worktrees",
            Style::default().fg(theme.muted),
        )]
    } else {
        rows
    };

    let height = (rows.len() as u16 + 2).min(area.height);
    let width = (area.width * 70 / 100).max(1).min(area.width);
    let rect = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused))
        .title(Span::styled(
            " Worktrees ",
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(Color::Reset));
    frame.render_widget(Clear, rect);
    frame.render_widget(Paragraph::new(rows).block(block), rect);
}
