//! Insertion and deletion operations for `TextInput`.

use super::TextInput;

impl TextInput {
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

    /// Delete from the cursor position back to the start of the current line.
    /// For single-line inputs this clears everything before the cursor.
    /// For multi-line inputs this deletes back to the previous newline.
    pub fn delete_to_line_start(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let line_start = if self.multiline {
            let before = &self.buffer[..self.cursor];
            before.rfind('\n').map_or(0, |pos| pos + 1)
        } else {
            0
        };
        self.buffer.drain(line_start..self.cursor);
        self.cursor = line_start;
    }

    /// Clear the buffer (Ctrl+A equivalent — select all then clear).
    pub fn select_all_and_clear(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
    }
}
