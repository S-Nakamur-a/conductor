//! Media rendering state for the viewer panel.
//!
//! Handles loading images via the `image` crate, rendering them to ANSI
//! strings via `aa_media::Renderer`, and converting the output to ratatui
//! `Line`s for display in the viewer panel.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use aa_media::renderer::{Mode, Renderer};

/// File extensions recognized as images.
const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp",
];

/// File extensions recognized as videos.
const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "webm", "avi", "mov", "mkv",
];

/// Check whether a file path has a media (image or video) extension.
pub fn is_media_file(path: &str) -> bool {
    is_image_file(path) || is_video_file(path)
}

/// Check whether a file path has an image extension.
pub fn is_image_file(path: &str) -> bool {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    IMAGE_EXTENSIONS.contains(&ext.as_str())
}

/// Check whether a file path has a video extension.
pub fn is_video_file(path: &str) -> bool {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    VIDEO_EXTENSIONS.contains(&ext.as_str())
}

/// Cached result of media rendering.
#[derive(Clone)]
pub enum MediaContent {
    /// Rendering is in progress.
    Loading,
    /// Successfully rendered lines.
    Rendered {
        lines: Vec<Line<'static>>,
        /// Image dimensions (width x height pixels).
        dimensions: (u32, u32),
        /// File size in bytes.
        file_size: u64,
    },
    /// Rendering failed; show this error message.
    Error(String),
}

/// State for media display in the viewer.
pub struct MediaState {
    /// The file path that was rendered (to detect when it changes).
    pub rendered_path: Option<String>,
    /// The terminal size (cols, rows) used for the last render.
    pub rendered_size: (u16, u16),
    /// Shared render result, updated by the background thread.
    pub content: Arc<Mutex<MediaContent>>,
}

impl Default for MediaState {
    fn default() -> Self {
        Self {
            rendered_path: None,
            rendered_size: (0, 0),
            content: Arc::new(Mutex::new(MediaContent::Loading)),
        }
    }
}

impl MediaState {
    /// Start rendering a media file in a background thread.
    ///
    /// If the file and size haven't changed, this is a no-op (cached result
    /// is reused).
    pub fn render_if_needed(&mut self, full_path: &Path, rel_path: &str, cols: u16, rows: u16) {
        let size = (cols, rows);

        // Use cached result if file and size haven't changed.
        if self.rendered_path.as_deref() == Some(rel_path) && self.rendered_size == size {
            return;
        }

        self.rendered_path = Some(rel_path.to_string());
        self.rendered_size = size;
        *self.content.lock().unwrap() = MediaContent::Loading;

        let path = full_path.to_path_buf();
        let content = Arc::clone(&self.content);

        thread::spawn(move || {
            let result = render_image_to_lines(&path, cols, rows);
            *content.lock().unwrap() = result;
        });
    }

    /// Invalidate the cache (e.g. when switching to a non-media file).
    pub fn clear(&mut self) {
        self.rendered_path = None;
        self.rendered_size = (0, 0);
    }
}

/// Render an image file to ratatui `Line`s using aa-media's Tile renderer.
fn render_image_to_lines(path: &Path, cols: u16, rows: u16) -> MediaContent {
    // Read file size.
    let file_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

    // Load the image.
    let img = match image::open(path) {
        Ok(img) => img.into_rgb8(),
        Err(e) => return MediaContent::Error(format!("Failed to load image: {e}")),
    };

    let src_w = img.width();
    let src_h = img.height();

    // Use Tile mode (half-block) for best quality with color.
    let mut renderer = Renderer::new(Mode::Tile, " .:-=+*#%@", true);

    // Compute target pixel size that fits the panel (leave room for border).
    let avail_cols = cols.saturating_sub(2);
    let avail_rows = rows.saturating_sub(3); // borders + info line
    let (tw, th) = renderer.target_size_fit(avail_cols, avail_rows, src_w, src_h);

    // Resize the image.
    let resized = image::imageops::resize(
        &img,
        tw as u32,
        th as u32,
        image::imageops::FilterType::Triangle,
    );

    // Render to ANSI string.
    let mut buf: Vec<u8> = Vec::with_capacity(1 << 16);
    if let Err(e) = renderer.render_frame(&mut buf, &resized, avail_cols) {
        return MediaContent::Error(format!("Render error: {e}"));
    }

    // Parse ANSI output through vt100 to extract styled cells.
    let lines = ansi_to_ratatui_lines(&buf, avail_cols, avail_rows);

    MediaContent::Rendered {
        lines,
        dimensions: (src_w, src_h),
        file_size,
    }
}

/// Convert ANSI-escaped bytes into ratatui `Line`s by parsing through vt100.
fn ansi_to_ratatui_lines(ansi_bytes: &[u8], cols: u16, rows: u16) -> Vec<Line<'static>> {
    let parser = vt100::Parser::new(rows, cols, 0);
    let mut parser = parser;
    parser.process(ansi_bytes);

    let screen = parser.screen();
    let mut lines = Vec::new();

    for row in 0..rows {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut current_text = String::new();
        let mut current_style = Style::default();
        let mut first = true;

        for col in 0..cols {
            let cell = screen.cell(row, col);
            let cell = match cell {
                Some(c) => c,
                None => continue,
            };

            let style = vt100_cell_to_style(cell);
            let ch = cell.contents();

            if first {
                current_style = style;
                first = false;
            }

            if style != current_style {
                if !current_text.is_empty() {
                    spans.push(Span::styled(
                        std::mem::take(&mut current_text),
                        current_style,
                    ));
                }
                current_style = style;
            }

            if ch.is_empty() {
                current_text.push(' ');
            } else {
                current_text.push_str(&ch);
            }
        }

        if !current_text.is_empty() {
            // Trim trailing spaces.
            let trimmed = current_text.trim_end();
            if !trimmed.is_empty() {
                spans.push(Span::styled(trimmed.to_string(), current_style));
            }
        }

        if !spans.is_empty() {
            lines.push(Line::from(spans));
        }
    }

    // Remove trailing empty lines.
    while lines.last().is_some_and(|l| l.spans.is_empty()) {
        lines.pop();
    }

    lines
}

/// Convert a vt100 cell's colors to a ratatui `Style`.
fn vt100_cell_to_style(cell: &vt100::Cell) -> Style {
    let mut style = Style::default();

    style = match cell.fgcolor() {
        vt100::Color::Default => style,
        vt100::Color::Idx(i) => style.fg(Color::Indexed(i)),
        vt100::Color::Rgb(r, g, b) => style.fg(Color::Rgb(r, g, b)),
    };

    style = match cell.bgcolor() {
        vt100::Color::Default => style,
        vt100::Color::Idx(i) => style.bg(Color::Indexed(i)),
        vt100::Color::Rgb(r, g, b) => style.bg(Color::Rgb(r, g, b)),
    };

    if cell.bold() {
        style = style.add_modifier(ratatui::style::Modifier::BOLD);
    }

    style
}
