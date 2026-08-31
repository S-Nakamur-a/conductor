//! テーマピッカーオーバーレイ — 実行時に UI テーマを切り替えるコンパクトなリストポップアップ。
//!
//! 各エントリはテーマ名と light/dark タグを表示する。上下キーでライブプレビュー付きの
//! 選択移動、Enter で確定・永続化、Esc で元に戻す。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};

use crate::app::App;
use crate::theme::Theme;

/// テーマピッカーオーバーレイを画面中央のモーダルポップアップとして描画する。
pub fn render_theme_picker_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.appearance.theme;
    let themes = &app.overlays.theme_picker.themes;

    // すべてのテーマが余裕を持って収まるようポップアップのサイズを決める。
    let popup_height = (themes.len() as u16 + 2).min(area.height.saturating_sub(4));
    let popup_width = 38_u16.min(area.width.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let block = Block::default()
        .title(" Switch Theme  ↑/↓ preview  Enter confirm  Esc revert ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent));

    let list_inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let selected = app.overlays.theme_picker.selected;
    let items: Vec<ListItem> = themes
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let tag = if Theme::from_name(name).light {
                " ☀ light"
            } else {
                " 🌙 dark"
            };
            let label = format!("  {name}{tag}");
            let style = if i == selected {
                Style::default()
                    .fg(theme.selected_fg)
                    .bg(theme.selected_bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };
            ListItem::new(Line::from(Span::styled(label, style)))
        })
        .collect();

    let list = List::new(items).highlight_style(
        Style::default()
            .bg(theme.selected_bg)
            .fg(theme.selected_fg)
            .add_modifier(Modifier::BOLD),
    );

    let mut state = ListState::default();
    state.select(Some(selected));
    frame.render_stateful_widget(list, list_inner, &mut state);
}
