//! Shared text-input rendering helpers used by the dashboard overlays:
//! cursor placement, block-cursor formatting, and multi-line word wrap with
//! cursor tracking.

use crate::text_input::TextInput;
use ratatui::Frame;
use ratatui::layout::{Position, Rect};

/// Set the terminal cursor position for IME at the cursor position within a
/// single-line `TextInput`.
pub(super) fn set_cursor_for_input(frame: &mut Frame, area: Rect, buffer: &TextInput) {
    let text_width = buffer.display_width_before_cursor() as u16;
    let cursor_x = area.x + text_width;
    let cursor_y = area.y;
    if cursor_x < area.x + area.width && cursor_y < area.y + area.height {
        frame.set_cursor_position(Position::new(cursor_x, cursor_y));
    }
}

/// Format a single-line `TextInput` with a block cursor at the cursor position.
pub(super) fn format_input_with_cursor(buffer: &TextInput) -> String {
    format!(
        "{}\u{2588}{}",
        buffer.text_before_cursor(),
        buffer.text_after_cursor()
    )
}

/// Wrap `text` into visual rows that are at most `width` display-columns wide,
/// hard-breaking long lines (and honouring explicit `\n`). Returns the wrapped
/// rows plus the (row, col) of `cursor_char` within them — so the caller can
/// place the cursor and scroll to keep it visible. This mirrors exactly what is
/// rendered (we draw these rows without ratatui's own `Wrap`), so the cursor
/// never drifts from the text the way it did when `Paragraph` re-wrapped behind
/// our back.
pub(super) fn wrap_with_cursor(
    text: &str,
    width: usize,
    cursor_char: char,
) -> (Vec<String>, usize, usize) {
    use unicode_width::UnicodeWidthChar;
    let width = width.max(1);
    let mut rows: Vec<String> = vec![String::new()];
    let mut cur_w = 0usize;
    let mut cursor_pos = (0usize, 0usize);
    for ch in text.chars() {
        if ch == '\n' {
            rows.push(String::new());
            cur_w = 0;
            continue;
        }
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if cur_w + cw > width && !rows.last().unwrap().is_empty() {
            rows.push(String::new());
            cur_w = 0;
        }
        if ch == cursor_char {
            cursor_pos = (rows.len() - 1, cur_w);
        }
        rows.last_mut().unwrap().push(ch);
        cur_w += cw;
    }
    (rows, cursor_pos.0, cursor_pos.1)
}

#[cfg(test)]
mod tests {
    use super::wrap_with_cursor;

    #[test]
    fn explicit_newlines_become_rows() {
        let (rows, r, c) = wrap_with_cursor("ab\ncd\u{2588}", 80, '\u{2588}');
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], "ab");
        // Cursor glyph sits at row 1, after "cd".
        assert_eq!((r, c), (1, 2));
    }

    #[test]
    fn long_line_hard_wraps_at_width_and_tracks_cursor() {
        // 10 chars, width 4 → rows of 4,4,2. Cursor glyph at the very end.
        let (rows, r, c) = wrap_with_cursor("0123456789\u{2588}", 4, '\u{2588}');
        assert_eq!(rows, vec!["0123", "4567", "89\u{2588}"]);
        assert_eq!((r, c), (2, 2));
    }

    #[test]
    fn wide_chars_do_not_split_across_the_boundary() {
        // Each CJK char is 2 cols wide; width 3 fits one per row.
        let (rows, _r, _c) = wrap_with_cursor("あい", 3, '\u{2588}');
        assert_eq!(rows, vec!["あ", "い"]);
    }

    #[test]
    fn empty_text_yields_one_row_and_origin_cursor() {
        let (rows, r, c) = wrap_with_cursor("\u{2588}", 80, '\u{2588}');
        assert_eq!(rows.len(), 1);
        assert_eq!((r, c), (0, 0));
    }
}
