//! パネル番号オーバーレイ — Alt+/ で切り替え、2秒後に自動的に消える。
//!
//! 各パネルの中央に大きな数字を表示し、Alt+N ショートカットを示す。

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use crate::app::{App, Focus};

/// パネル情報: (領域, ラベル, 対応する Focus)。
struct PanelInfo {
    area: Rect,
    number: &'static str,
    label: &'static str,
    is_focused: bool,
}

/// すべてのパネルにパネル番号オーバーレイを描画する。
pub fn render_panel_overlay(frame: &mut Frame, app: &App) {
    let columns = app.layout.cache.columns;
    let terminal_split = app.layout.cache.terminal_split;
    let explorer_mid_y = app.layout.cache.explorer_mid_y;

    // Explorer カラム（columns[1]）を上（ファイルツリー）と下（差分リスト）に分割する。
    let explorer_col = columns[1];
    let explorer_top = Rect::new(
        explorer_col.x,
        explorer_col.y,
        explorer_col.width,
        explorer_mid_y.saturating_sub(explorer_col.y),
    );
    let explorer_bottom = Rect::new(
        explorer_col.x,
        explorer_mid_y,
        explorer_col.width,
        explorer_col.height.saturating_sub(explorer_top.height),
    );

    let is_explorer_focused = app.focus == Focus::Explorer;
    let on_diff_list = app.explorer.focus_on_diff_list;

    let panels = [
        PanelInfo {
            area: columns[0],
            number: "1",
            label: "Worktree",
            is_focused: app.focus == Focus::Worktree,
        },
        PanelInfo {
            area: explorer_top,
            number: "2",
            label: "Explorer",
            is_focused: is_explorer_focused && !on_diff_list,
        },
        PanelInfo {
            area: explorer_bottom,
            number: "3",
            label: "Diff List",
            is_focused: is_explorer_focused && on_diff_list,
        },
        PanelInfo {
            area: columns[2],
            number: "4",
            label: "Diff Viewer",
            is_focused: app.focus == Focus::Viewer,
        },
        PanelInfo {
            area: terminal_split[0],
            number: "5",
            label: "Claude",
            is_focused: app.focus == Focus::TerminalClaude,
        },
        PanelInfo {
            area: terminal_split[1],
            number: "6",
            label: "Shell",
            is_focused: app.focus == Focus::TerminalShell,
        },
    ];

    for panel in &panels {
        if panel.area.width < 3 || panel.area.height < 3 {
            continue;
        }
        render_single_panel_overlay(frame, panel, &app.theme);
    }
}

fn render_single_panel_overlay(frame: &mut Frame, panel: &PanelInfo, theme: &crate::theme::Theme) {
    let area = panel.area;
    let is_focused = panel.is_focused;

    // 下の内容が透けて見えないようクリアする。
    frame.render_widget(Clear, area);

    // 背景色: フォーカス中のパネルはアクセントカラー（暗め）、
    // フォーカスされていないパネルは暗いオーバーレイにする。
    let bg = if is_focused {
        Color::Rgb(40, 60, 80)
    } else {
        Color::Rgb(25, 25, 35)
    };

    let border_color = if is_focused {
        theme.accent
    } else {
        theme.border_unfocused
    };

    let block = Block::bordered()
        .border_style(Style::default().fg(border_color))
        .style(Style::default().bg(bg));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    // 数字 + ラベルのテキストを作成し、垂直方向に中央揃えする。
    let number_style = Style::default()
        .fg(if is_focused {
            theme.fg
        } else {
            Color::Rgb(180, 180, 200)
        })
        .add_modifier(Modifier::BOLD);

    let label_style = Style::default().fg(if is_focused {
        Color::Rgb(200, 200, 220)
    } else {
        Color::Rgb(100, 100, 120)
    });

    let lines = vec![
        Line::from(Span::styled(panel.number, number_style)),
        Line::from(Span::styled(panel.label, label_style)),
    ];

    // 2行分のコンテンツを垂直方向に中央揃えする。
    let content_height = lines.len() as u16;
    let top_pad = inner.height.saturating_sub(content_height) / 2;
    let text_area = Rect::new(
        inner.x,
        inner.y + top_pad,
        inner.width,
        content_height.min(inner.height),
    );

    let paragraph = Paragraph::new(lines).alignment(Alignment::Center);
    frame.render_widget(paragraph, text_area);
}
