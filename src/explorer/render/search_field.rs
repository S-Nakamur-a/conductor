//! Explorer 下部に重ねるファイル名検索の入力欄。
//!
//! 表示するかどうかと中身の文字列は Viewer の検索状態を写した
//! [crate::explorer::ctx::Paint::search] に従う。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Span;

use crate::explorer::ctx::{Ctx, Paint};

/// 検索欄を描画し、置くべき端末カーソルの位置を返す。検索非アクティブなら
/// 何も描かず None を返す。
pub(super) fn render(
    frame: &mut Frame,
    area: Rect,
    ctx: &Ctx,
    paint: &Paint,
) -> Option<(u16, u16)> {
    let query = paint.search?;

    let height = 1_u16;
    let y = area.y + area.height.saturating_sub(height + 1);
    let search_area = Rect::new(area.x + 1, y, area.width.saturating_sub(2), height);

    frame.render_widget(ratatui::widgets::Clear, search_area);

    let text = format!("/{query}\u{2588}");
    let paragraph = ratatui::widgets::Paragraph::new(Span::styled(
        text,
        Style::default().fg(ctx.theme.search_match_fg),
    ));
    frame.render_widget(paragraph, search_area);

    // 先頭の '/' とクエリ文字列の分だけ +1。カーソルは常に末尾に置く —
    // Paint::search は文字列全体しか運ばないので、クエリ内を左右に動かした
    // 場合のカーソル位置は再現できない（キー入力側もその区別を持てば直せる）。
    let cursor_x = search_area.x + 1 + unicode_width::UnicodeWidthStr::width(query) as u16;
    (cursor_x < search_area.x + search_area.width).then_some((cursor_x, search_area.y))
}
