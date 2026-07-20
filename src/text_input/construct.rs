//! Construction and read-only state accessors for `TextInput`.

use unicode_width::UnicodeWidthStr;

use super::TextInput;

impl TextInput {
    /// Create a new single-line text input.
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            cursor: 0,
            multiline: false,
        }
    }

    /// Create a new multi-line text input.
    pub fn new_multiline() -> Self {
        Self {
            buffer: String::new(),
            cursor: 0,
            multiline: true,
        }
    }

    /// Clear the buffer and reset cursor to 0.
    pub fn clear(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
    }

    /// Replace the entire buffer content and move cursor to end.
    pub fn set_text(&mut self, text: &str) {
        self.buffer = text.to_string();
        self.cursor = self.buffer.len();
    }

    /// Return a reference to the buffer content.
    pub fn text(&self) -> &str {
        &self.buffer
    }

    /// Return the text before the cursor.
    pub fn text_before_cursor(&self) -> &str {
        &self.buffer[..self.cursor]
    }

    /// Return the text after the cursor.
    pub fn text_after_cursor(&self) -> &str {
        &self.buffer[self.cursor..]
    }

    /// Calculate the (row, col) of the cursor for multi-line display.
    /// Row and col are 0-indexed. Col is in display width (unicode).
    pub fn cursor_row_col(&self) -> (usize, usize) {
        let before = self.text_before_cursor();
        let row = before.matches('\n').count();
        let last_line = before.rsplit('\n').next().unwrap_or(before);
        let col = UnicodeWidthStr::width(last_line);
        (row, col)
    }

    /// Return the display width of text before the cursor on the current line.
    pub fn display_width_before_cursor(&self) -> usize {
        let before = self.text_before_cursor();
        let last_line = before.rsplit('\n').next().unwrap_or(before);
        UnicodeWidthStr::width(last_line)
    }
}
