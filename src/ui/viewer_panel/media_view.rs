//! ビューアパネルのメディアファイル（画像/動画）表示モード。
//! ハーフブロックによるカラーの ASCII アートとして描画する。

use crate::app::App;
use crate::media_state::MediaContent;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

/// ビューアパネル内にメディアファイル（画像/動画）を ASCII アートとして描画する。
pub(super) fn render_media_view(frame: &mut Frame, area: Rect, app: &App, block: Block<'_>) {
    let theme = &app.theme;
    let vs = &app.viewer_state;

    // ロックが poison していてもパニックせず復旧する。デコードスレッドは描画中このミューテックス
    // を保持しており、そこでのパニック（壊れたメディア）が次フレームで TUI 全体を巻き込んではならない。
    let content = vs
        .media_state
        .content
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();

    match content {
        MediaContent::Loading => {
            let loading = Paragraph::new("Loading media...")
                .style(Style::default().fg(theme.muted))
                .block(block);
            frame.render_widget(loading, area);
        }
        MediaContent::Rendered {
            lines,
            dimensions,
            file_size,
        } => {
            frame.render_widget(ratatui::widgets::Clear, area);

            let inner = block.inner(area);
            frame.render_widget(block, area);

            // 最終行は情報バー用に確保する。
            let media_height = inner.height.saturating_sub(1) as usize;
            let media_area = Rect::new(inner.x, inner.y, inner.width, media_height as u16);
            let info_area = Rect::new(inner.x, inner.y + media_height as u16, inner.width, 1);

            // メディアの行を描画する。
            let visible_lines: Vec<Line> = lines.into_iter().take(media_height).collect();
            let paragraph = Paragraph::new(visible_lines);
            frame.render_widget(paragraph, media_area);

            render_media_info_bar(frame, info_area, dimensions, file_size, theme);
        }
        MediaContent::Error(msg) => {
            let error = Paragraph::new(msg)
                .style(Style::default().fg(theme.error))
                .block(block);
            frame.render_widget(error, area);
        }
    }
}

/// 画像の下にメディア情報バー（サイズ + ファイルサイズ）を描画する。
fn render_media_info_bar(
    frame: &mut Frame,
    info_area: Rect,
    dimensions: (u32, u32),
    file_size: u64,
    theme: &crate::theme::Theme,
) {
    let size_str = if file_size >= 1_048_576 {
        format!("{:.1} MB", file_size as f64 / 1_048_576.0)
    } else if file_size >= 1024 {
        format!("{:.1} KB", file_size as f64 / 1024.0)
    } else {
        format!("{file_size} B")
    };
    let info = format!(" {}x{} | {} ", dimensions.0, dimensions.1, size_str);
    let info_widget = Paragraph::new(Span::styled(info, Style::default().fg(theme.muted)));
    frame.render_widget(info_widget, info_area);
}
