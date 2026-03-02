//! Reusable text input buffer with cursor movement and editing.
//!
//! Provides a `TextInput` struct that supports cursor navigation,
//! insertion at the cursor position, forward/backward deletion,
//! word-level movement, and clipboard paste.

use std::fmt;
use std::ops::Deref;

use unicode_width::UnicodeWidthStr;

/// A text input buffer with cursor position tracking.
///
/// Supports single-line and multi-line modes, cursor movement,
/// insertion/deletion at cursor position, and word-level navigation.
#[derive(Clone, Debug)]
pub struct TextInput {
    buffer: String,
    /// Cursor position as a byte offset into `buffer`.
    cursor: usize,
    /// Whether this input supports multi-line editing.
    multiline: bool,
}

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

    /// Insert a character at the cursor position.
    pub fn insert_char(&mut self, c: char) {
        self.buffer.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    /// Insert a string at the cursor position.
    /// For single-line inputs, newlines are stripped.
    pub fn insert_str(&mut self, s: &str) {
        if self.multiline {
            self.buffer.insert_str(self.cursor, s);
            self.cursor += s.len();
        } else {
            // Strip newlines for single-line input.
            let cleaned: String = s.chars().filter(|&c| c != '\n' && c != '\r').collect();
            self.buffer.insert_str(self.cursor, &cleaned);
            self.cursor += cleaned.len();
        }
    }

    /// Delete the character before the cursor (Backspace).
    pub fn delete_backward(&mut self) {
        if self.cursor == 0 {
            return;
        }
        // Find the previous char boundary.
        let prev = self.prev_char_boundary();
        self.buffer.drain(prev..self.cursor);
        self.cursor = prev;
    }

    /// Delete the character after the cursor (Delete key).
    pub fn delete_forward(&mut self) {
        if self.cursor >= self.buffer.len() {
            return;
        }
        let next = self.next_char_boundary();
        self.buffer.drain(self.cursor..next);
    }

    /// Move cursor one character to the left.
    pub fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor = self.prev_char_boundary();
        }
    }

    /// Move cursor one character to the right.
    pub fn move_right(&mut self) {
        if self.cursor < self.buffer.len() {
            self.cursor = self.next_char_boundary();
        }
    }

    /// Move cursor to the beginning of the line (Home).
    pub fn move_home(&mut self) {
        if self.multiline {
            // Move to the start of the current line.
            let before = &self.buffer[..self.cursor];
            if let Some(nl) = before.rfind('\n') {
                self.cursor = nl + 1;
            } else {
                self.cursor = 0;
            }
        } else {
            self.cursor = 0;
        }
    }

    /// Move cursor to the end of the line (End).
    pub fn move_end(&mut self) {
        if self.multiline {
            // Move to the end of the current line.
            let after = &self.buffer[self.cursor..];
            if let Some(nl) = after.find('\n') {
                self.cursor += nl;
            } else {
                self.cursor = self.buffer.len();
            }
        } else {
            self.cursor = self.buffer.len();
        }
    }

    /// Move cursor one word to the left (Ctrl+Left / Alt+Left).
    pub fn move_word_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let bytes = self.buffer.as_bytes();
        let mut pos = self.cursor;
        // Skip whitespace/punctuation to the left.
        while pos > 0 && !bytes[pos - 1].is_ascii_alphanumeric() {
            pos -= 1;
            // Align to char boundary.
            while pos > 0 && !self.buffer.is_char_boundary(pos) {
                pos -= 1;
            }
        }
        // Skip word characters to the left.
        while pos > 0 && bytes[pos - 1].is_ascii_alphanumeric() {
            pos -= 1;
        }
        self.cursor = pos;
    }

    /// Move cursor one word to the right (Ctrl+Right / Alt+Right).
    pub fn move_word_right(&mut self) {
        let len = self.buffer.len();
        if self.cursor >= len {
            return;
        }
        let bytes = self.buffer.as_bytes();
        let mut pos = self.cursor;
        // Skip word characters to the right.
        while pos < len && bytes[pos].is_ascii_alphanumeric() {
            pos += 1;
        }
        // Skip whitespace/punctuation to the right.
        while pos < len && !bytes[pos].is_ascii_alphanumeric() {
            pos += 1;
        }
        self.cursor = pos;
    }

    /// Clear the buffer (Ctrl+A equivalent — select all then clear).
    pub fn select_all_and_clear(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
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

    // ── Private helpers ─────────────────────────────────────────────────

    /// Find the byte position of the previous character boundary.
    fn prev_char_boundary(&self) -> usize {
        let mut pos = self.cursor;
        if pos == 0 {
            return 0;
        }
        pos -= 1;
        while pos > 0 && !self.buffer.is_char_boundary(pos) {
            pos -= 1;
        }
        pos
    }

    /// Find the byte position of the next character boundary.
    fn next_char_boundary(&self) -> usize {
        let mut pos = self.cursor + 1;
        while pos < self.buffer.len() && !self.buffer.is_char_boundary(pos) {
            pos += 1;
        }
        pos
    }
}

impl Default for TextInput {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for TextInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.buffer)
    }
}

impl Deref for TextInput {
    type Target = str;
    fn deref(&self) -> &str {
        &self.buffer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_and_empty() {
        let ti = TextInput::new();
        assert!(ti.is_empty());
        assert_eq!(ti.text(), "");
        assert_eq!(ti.cursor, 0);
    }

    #[test]
    fn test_insert_char() {
        let mut ti = TextInput::new();
        ti.insert_char('h');
        ti.insert_char('i');
        assert_eq!(ti.text(), "hi");
        assert_eq!(ti.cursor, 2);
    }

    #[test]
    fn test_insert_at_cursor() {
        let mut ti = TextInput::new();
        ti.insert_char('a');
        ti.insert_char('c');
        ti.move_left();
        ti.insert_char('b');
        assert_eq!(ti.text(), "abc");
    }

    #[test]
    fn test_delete_backward() {
        let mut ti = TextInput::new();
        ti.set_text("abc");
        ti.delete_backward();
        assert_eq!(ti.text(), "ab");
        ti.move_home();
        ti.delete_backward(); // no-op at start
        assert_eq!(ti.text(), "ab");
    }

    #[test]
    fn test_delete_forward() {
        let mut ti = TextInput::new();
        ti.set_text("abc");
        ti.move_home();
        ti.delete_forward();
        assert_eq!(ti.text(), "bc");
        ti.move_end();
        ti.delete_forward(); // no-op at end
        assert_eq!(ti.text(), "bc");
    }

    #[test]
    fn test_move_left_right() {
        let mut ti = TextInput::new();
        ti.set_text("abc");
        assert_eq!(ti.cursor, 3);
        ti.move_left();
        assert_eq!(ti.cursor, 2);
        ti.move_left();
        assert_eq!(ti.cursor, 1);
        ti.move_right();
        assert_eq!(ti.cursor, 2);
        // Move left past start
        ti.move_home();
        ti.move_left();
        assert_eq!(ti.cursor, 0);
        // Move right past end
        ti.move_end();
        ti.move_right();
        assert_eq!(ti.cursor, 3);
    }

    #[test]
    fn test_move_home_end() {
        let mut ti = TextInput::new();
        ti.set_text("hello");
        ti.move_home();
        assert_eq!(ti.cursor, 0);
        ti.move_end();
        assert_eq!(ti.cursor, 5);
    }

    #[test]
    fn test_multibyte_chars() {
        let mut ti = TextInput::new();
        ti.insert_char('あ');
        ti.insert_char('い');
        ti.insert_char('う');
        assert_eq!(ti.text(), "あいう");
        assert_eq!(ti.cursor, 9); // 3 bytes per char
        ti.move_left();
        assert_eq!(ti.cursor, 6);
        ti.delete_backward();
        assert_eq!(ti.text(), "あう");
        ti.delete_forward();
        assert_eq!(ti.text(), "あ");
    }

    #[test]
    fn test_word_movement() {
        let mut ti = TextInput::new();
        ti.set_text("hello world foo");
        ti.move_home();
        ti.move_word_right();
        assert_eq!(ti.cursor, 6); // after "hello "
        ti.move_word_right();
        assert_eq!(ti.cursor, 12); // after "world "
        ti.move_word_left();
        assert_eq!(ti.cursor, 6); // back to "world"
        ti.move_word_left();
        assert_eq!(ti.cursor, 0); // back to start
    }

    #[test]
    fn test_select_all_and_clear() {
        let mut ti = TextInput::new();
        ti.set_text("some text");
        ti.select_all_and_clear();
        assert!(ti.is_empty());
        assert_eq!(ti.cursor, 0);
    }

    #[test]
    fn test_text_before_after_cursor() {
        let mut ti = TextInput::new();
        ti.set_text("abcdef");
        ti.move_home();
        ti.move_right();
        ti.move_right();
        ti.move_right();
        assert_eq!(ti.text_before_cursor(), "abc");
        assert_eq!(ti.text_after_cursor(), "def");
    }

    #[test]
    fn test_multiline_cursor_row_col() {
        let mut ti = TextInput::new_multiline();
        ti.set_text("hello\nworld\nfoo");
        let (row, col) = ti.cursor_row_col();
        assert_eq!(row, 2);
        assert_eq!(col, 3); // "foo" width
    }

    #[test]
    fn test_multiline_home_end() {
        let mut ti = TextInput::new_multiline();
        ti.set_text("line1\nline2\nline3");
        // Cursor at end of "line3"
        ti.move_home();
        assert_eq!(ti.text_before_cursor(), "line1\nline2\n");
        ti.move_end();
        assert_eq!(ti.text_after_cursor(), "");
    }

    #[test]
    fn test_insert_str_single_line() {
        let mut ti = TextInput::new();
        ti.insert_str("hello\nworld");
        assert_eq!(ti.text(), "helloworld"); // newlines stripped
    }

    #[test]
    fn test_insert_str_multiline() {
        let mut ti = TextInput::new_multiline();
        ti.insert_str("hello\nworld");
        assert_eq!(ti.text(), "hello\nworld");
    }

    #[test]
    fn test_set_text_moves_cursor_to_end() {
        let mut ti = TextInput::new();
        ti.set_text("hello");
        assert_eq!(ti.cursor, 5);
        ti.set_text("hi");
        assert_eq!(ti.cursor, 2);
    }

    #[test]
    fn test_display_width_japanese() {
        let mut ti = TextInput::new();
        ti.set_text("あいう");
        // Each Japanese char has display width 2, so total = 6
        assert_eq!(ti.display_width_before_cursor(), 6);
        ti.move_left();
        assert_eq!(ti.display_width_before_cursor(), 4);
    }

    #[test]
    fn test_deref() {
        let ti = TextInput::new();
        assert!(ti.is_empty()); // str::is_empty via Deref
        let mut ti = TextInput::new();
        ti.set_text("Hello World");
        assert!(ti.contains("World")); // str::contains via Deref
        assert_eq!(ti.to_lowercase(), "hello world");
    }
}
