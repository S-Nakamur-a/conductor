//! Cursor navigation for `TextInput`: character, line, and word movement.

use super::TextInput;

impl TextInput {
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
}
