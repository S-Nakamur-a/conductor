//! edit と movement が共有する、非公開の文字境界ヘルパー。

use super::TextInput;

impl TextInput {
    /// 直前の文字境界のバイト位置を求める。
    pub(super) fn prev_char_boundary(&self) -> usize {
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

    /// 直後の文字境界のバイト位置を求める。
    pub(super) fn next_char_boundary(&self) -> usize {
        let mut pos = self.cursor + 1;
        while pos < self.buffer.len() && !self.buffer.is_char_boundary(pos) {
            pos += 1;
        }
        pos
    }
}
