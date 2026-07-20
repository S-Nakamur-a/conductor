//! Media file (image/video) rendering mode of the viewer panel — ASCII art or
//! terminal-graphics-protocol pixel display, depending on `term_caps`.

use crate::app::App;
use crate::media_state::MediaContent;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

/// Render media file (image/video) as ASCII art in the viewer panel.
pub(super) fn render_media_view(frame: &mut Frame, area: Rect, app: &App, block: Block<'_>) {
    let theme = &app.theme;
    let vs = &app.viewer_state;

    // Recover from a poisoned lock instead of panicking: the decode thread
    // holds this mutex while rendering, so a panic there (malformed media)
    // must not take down the whole TUI on the next frame.
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

            // Reserve last line for info bar.
            let media_height = inner.height.saturating_sub(1) as usize;
            let media_area = Rect::new(inner.x, inner.y, inner.width, media_height as u16);
            let info_area = Rect::new(inner.x, inner.y + media_height as u16, inner.width, 1);

            // Render the media lines.
            let visible_lines: Vec<Line> = lines.into_iter().take(media_height).collect();
            let paragraph = Paragraph::new(visible_lines);
            frame.render_widget(paragraph, media_area);

            render_media_info_bar(frame, info_area, dimensions, file_size, theme);
        }
        MediaContent::Pixel {
            protocol,
            dimensions,
            file_size,
        } => {
            frame.render_widget(ratatui::widgets::Clear, area);

            let inner = block.inner(area);
            frame.render_widget(block, area);

            // Reserve last line for info bar.
            let media_height = inner.height.saturating_sub(1);
            let media_area = Rect::new(inner.x, inner.y, inner.width, media_height);
            let info_area = Rect::new(inner.x, inner.y + media_height, inner.width, 1);

            // Pixel-quality image via the terminal graphics protocol. The
            // escape payload is embedded in the buffer cells, so ratatui's
            // diffing only re-transmits it when the cells actually change.
            frame.render_widget(ratatui_image::Image::new(protocol.as_ref()), media_area);

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

/// Render the media info bar (dimensions + file size) under the image.
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
