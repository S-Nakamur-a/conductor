//! エクスプローラ下部に表示するパネル内ファイル名検索の入力欄。

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use ratatui::text::Span;

/// 指定領域の下部に検索入力欄を描画する。
pub(super) fn render_search_box(
    frame: &mut Frame,
    area: Rect,
    query: &crate::text_input::TextInput,
    theme: &crate::theme::Theme,
    suppress_cursor: bool,
) {
    let height = 1_u16;
    let y = area.y + area.height.saturating_sub(height + 1);
    let search_area = Rect::new(area.x + 1, y, area.width.saturating_sub(2), height);

    frame.render_widget(ratatui::widgets::Clear, search_area);

    let text = format!(
        "/{}\u{2588}{}",
        query.text_before_cursor(),
        query.text_after_cursor()
    );
    let paragraph = ratatui::widgets::Paragraph::new(Span::styled(
        text,
        Style::default().fg(theme.search_match_fg),
    ));
    frame.render_widget(paragraph, search_area);
    if !suppress_cursor {
        // 先頭の '/' 文字の分だけ +1
        let cursor_x = search_area.x + 1 + query.display_width_before_cursor() as u16;
        if cursor_x < search_area.x + search_area.width {
            frame.set_cursor_position(Position::new(cursor_x, search_area.y));
        }
    }
}
