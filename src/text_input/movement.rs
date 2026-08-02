//! TextInput のカーソルナビゲーション: 文字単位・行単位・単語単位の移動。

use super::TextInput;

impl TextInput {
    /// カーソルを1文字左へ移動する。
    pub fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor = self.prev_char_boundary();
        }
    }

    /// カーソルを1文字右へ移動する。
    pub fn move_right(&mut self) {
        if self.cursor < self.buffer.len() {
            self.cursor = self.next_char_boundary();
        }
    }

    /// カーソルを行頭へ移動する（Home）。
    pub fn move_home(&mut self) {
        if self.multiline {
            // 現在行の先頭へ移動する。
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

    /// カーソルを行末へ移動する（End）。
    pub fn move_end(&mut self) {
        if self.multiline {
            // 現在行の末尾へ移動する。
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

    /// カーソルを1単語分左へ移動する（Ctrl+Left / Alt+Left）。
    pub fn move_word_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let bytes = self.buffer.as_bytes();
        let mut pos = self.cursor;
        // 左方向に空白/記号をスキップする。
        while pos > 0 && !bytes[pos - 1].is_ascii_alphanumeric() {
            pos -= 1;
            // 文字境界に合わせる。
            while pos > 0 && !self.buffer.is_char_boundary(pos) {
                pos -= 1;
            }
        }
        // 左方向に単語文字をスキップする。
        while pos > 0 && bytes[pos - 1].is_ascii_alphanumeric() {
            pos -= 1;
        }
        self.cursor = pos;
    }

    /// カーソルを1単語分右へ移動する（Ctrl+Right / Alt+Right）。
    pub fn move_word_right(&mut self) {
        let len = self.buffer.len();
        if self.cursor >= len {
            return;
        }
        let bytes = self.buffer.as_bytes();
        let mut pos = self.cursor;
        // 右方向に単語文字をスキップする。
        while pos < len && bytes[pos].is_ascii_alphanumeric() {
            pos += 1;
        }
        // 右方向に空白/記号をスキップする。
        while pos < len && !bytes[pos].is_ascii_alphanumeric() {
            pos += 1;
        }
        self.cursor = pos;
    }
}
