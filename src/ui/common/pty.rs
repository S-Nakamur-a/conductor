//! vt100 PTY screen snapshotting and rendering into cached ratatui `Line`s.

use std::sync::{Arc, Mutex};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::theme::Theme;

/// Cached PTY render output to avoid expensive vt100 snapshots every frame.
///
/// When a terminal panel is not focused, we reuse the previously built
/// ratatui `Line` data instead of re-locking the vt100 parser mutex and
/// copying thousands of cells.
#[derive(Default)]
pub struct PtyRenderCache {
    pub lines: Vec<Line<'static>>,
    pub effective_offset: usize,
    /// Cursor position (row, col) from the vt100 parser, used for IME positioning.
    pub cursor_position: Option<(u16, u16)>,
}

/// A snapshot of a single cell's content and style, extracted from the vt100 screen.
struct CellSnapshot {
    text: String,
    style: Style,
}

/// A snapshot of the vt100 screen contents, captured while holding the lock
/// so that the lock can be released before the (slower) ratatui rendering step.
struct ScreenSnapshot {
    rows: Vec<Vec<CellSnapshot>>,
    effective_offset: usize,
    /// Cursor position (row, col) from the vt100 parser.
    cursor_position: (u16, u16),
}

/// Take a point-in-time snapshot of the vt100 screen contents.
///
/// Uses `try_lock` to avoid blocking when the PTY reader thread holds
/// the mutex. Returns `None` if the lock is contended — the caller
/// should reuse the previous cached render in that case.
fn snapshot_screen(
    screen_arc: &Arc<Mutex<vt100::Parser>>,
    scroll_offset: usize,
    max_rows: u16,
    max_cols: u16,
) -> Option<ScreenSnapshot> {
    let mut parser = match screen_arc.try_lock() {
        Ok(p) => p,
        Err(std::sync::TryLockError::WouldBlock) => return None,
        Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
    };

    let is_alt_screen = parser.screen().alternate_screen();
    let requested_offset = if is_alt_screen { 0 } else { scroll_offset };

    parser.set_scrollback(requested_offset);
    // vt100 internally clamps to the actual scrollback buffer length.
    // Read back the effective offset so our cache reflects the real position.
    let effective_offset = parser.screen().scrollback();

    let screen = parser.screen();
    let (rows, cols) = screen.size();

    // Debug: log alternate screen state periodically.
    if is_alt_screen {
        let has_content = (0..rows.min(5)).any(|r| {
            (0..cols).any(|c| {
                if let Some(cell) = screen.cell(r, c) {
                    let ch = cell.contents();
                    !ch.is_empty() && ch != " "
                } else {
                    false
                }
            })
        });
        let cursor = screen.cursor_position();
        log::debug!(
            "ALT_SCREEN render: has_content={has_content}, size=({rows},{cols}), area=({max_rows},{max_cols}) cursor=({},{})",
            cursor.0,
            cursor.1,
        );
    }

    // Extract cell data into local snapshot.
    let mut snapshot_rows: Vec<Vec<CellSnapshot>> = Vec::with_capacity(rows.min(max_rows) as usize);
    for row in 0..rows.min(max_rows) {
        let mut row_cells: Vec<CellSnapshot> = Vec::new();
        for col in 0..cols.min(max_cols) {
            let Some(cell) = screen.cell(row, col) else {
                break;
            };
            row_cells.push(CellSnapshot {
                text: cell.contents(),
                style: vt100_cell_to_style(cell),
            });
        }
        snapshot_rows.push(row_cells);
    }

    // Capture cursor position before restoring scrollback.
    let cursor = screen.cursor_position();
    let cursor_position = (cursor.0, cursor.1);

    // Restore live view so other readers see the current screen.
    parser.set_scrollback(0);

    // Lock is dropped here when `parser` goes out of scope.
    Some(ScreenSnapshot {
        rows: snapshot_rows,
        effective_offset,
        cursor_position,
    })
}

/// Build ratatui `Line`s from a vt100 PTY screen snapshot.
///
/// This is the expensive operation: it locks the vt100 parser mutex,
/// copies cell data, then builds styled `Line` objects. The result can
/// be cached in a [`PtyRenderCache`] and reused across frames.
///
/// Returns `None` if the vt100 parser mutex is currently held by the
/// PTY reader thread. The caller should keep using the previous cache
/// instead of blocking the main thread.
pub fn build_pty_lines(
    screen_arc: &Arc<Mutex<vt100::Parser>>,
    scroll_offset: usize,
    max_rows: u16,
    max_cols: u16,
) -> Option<PtyRenderCache> {
    let snapshot = snapshot_screen(screen_arc, scroll_offset, max_rows, max_cols)?;
    let lines = lines_from_snapshot(&snapshot);
    let cursor_position = if snapshot.effective_offset == 0 {
        Some(snapshot.cursor_position)
    } else {
        None
    };
    Some(PtyRenderCache {
        lines,
        effective_offset: snapshot.effective_offset,
        cursor_position,
    })
}

/// Render previously built PTY lines from a [`PtyRenderCache`].
///
/// This is cheap: the cached `Line`s are blitted straight into the frame
/// buffer by reference. (It used to `clone()` the whole line vector into a
/// `Paragraph` — a full deep copy of every span string, twice per frame at
/// the terminal-focus tick rate, for zero benefit.)
pub fn render_pty_cached(frame: &mut Frame, area: Rect, cache: &PtyRenderCache, theme: &Theme) {
    // Clear first: when scrolled back the snapshot can have fewer/shorter lines
    // than the live view, and bare line blitting leaves the uncovered cells
    // showing the previous frame's text (the "scrollback bleed"). Mirrors the
    // viewer panel, which clears for the same reason.
    frame.render_widget(ratatui::widgets::Clear, area);
    let buf = frame.buffer_mut();
    for (i, line) in cache.lines.iter().enumerate().take(area.height as usize) {
        buf.set_line(area.x, area.y + i as u16, line, area.width);
    }

    if cache.effective_offset > 0 {
        let indicator = Line::from(Span::styled(
            format!(
                " ↑ scrollback ({} lines — Shift+End to return) ",
                cache.effective_offset
            ),
            Style::default().fg(theme.selected_fg).bg(theme.accent),
        ));
        frame.render_widget(Paragraph::new(indicator), Rect { height: 1, ..area });
    }
}

/// Build `Vec<Line<'static>>` from a `ScreenSnapshot`.
fn lines_from_snapshot(snapshot: &ScreenSnapshot) -> Vec<Line<'static>> {
    let mut text_lines: Vec<Line> = Vec::new();
    for row_cells in &snapshot.rows {
        let mut spans: Vec<Span> = Vec::new();
        let mut current_text = String::new();
        let mut current_style = Style::default();

        let mut skip_cols: usize = 0;
        for cell in row_cells {
            if skip_cols > 0 {
                skip_cols -= 1;
                continue;
            }
            let ch = &cell.text;
            let style = cell.style;

            if style != current_style && !current_text.is_empty() {
                spans.push(Span::styled(
                    std::mem::take(&mut current_text),
                    current_style,
                ));
                current_style = style;
            }
            if ch.is_empty() {
                current_text.push(' ');
            } else {
                let w = UnicodeWidthStr::width(ch.as_str());
                if w > 1 {
                    skip_cols = w - 1;
                }
                current_text.push_str(ch);
            }
        }
        if !current_text.is_empty() {
            spans.push(Span::styled(current_text, current_style));
        }
        text_lines.push(Line::from(spans));
    }
    text_lines
}

fn vt100_color_to_ratatui(color: vt100::Color) -> Color {
    match color {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

fn vt100_cell_to_style(cell: &vt100::Cell) -> Style {
    let mut style = Style::default();
    style = style.fg(vt100_color_to_ratatui(cell.fgcolor()));
    style = style.bg(vt100_color_to_ratatui(cell.bgcolor()));
    if cell.bold() {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.italic() {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell.underline() {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if cell.inverse() {
        style = style.add_modifier(Modifier::REVERSED);
    }
    style
}
