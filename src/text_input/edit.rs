//! TextInput の挿入・削除操作。

use super::TextInput;

impl TextInput {
    /// カーソル位置に1文字挿入する。
    pub fn insert_char(&mut self, c: char) {
        self.buffer.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    /// カーソル位置に文字列を挿入する。
    /// 単一行入力では改行を取り除く。
    pub fn insert_str(&mut self, s: &str) {
        if self.multiline {
            self.buffer.insert_str(self.cursor, s);
            self.cursor += s.len();
        } else {
            // 単一行入力なので改行を取り除く。
            let cleaned: String = s.chars().filter(|&c| c != '\n' && c != '\r').collect();
            self.buffer.insert_str(self.cursor, &cleaned);
            self.cursor += cleaned.len();
        }
    }

    /// カーソルの直前の文字を削除する（Backspace）。
    pub fn delete_backward(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let prev = self.prev_char_boundary();
        self.buffer.drain(prev..self.cursor);
        self.cursor = prev;
    }

    /// カーソルの直後の文字を削除する（Delete キー）。
    pub fn delete_forward(&mut self) {
        if self.cursor >= self.buffer.len() {
            return;
        }
        let next = self.next_char_boundary();
        self.buffer.drain(self.cursor..next);
    }

    /// カーソル位置から現在行の先頭まで削除する。
    /// 単一行入力ではカーソルより前を全て消す。
    /// 複数行入力では直前の改行まで消す。
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

    /// バッファをクリアする（Ctrl+A 相当 — 全選択してクリア）。
    pub fn select_all_and_clear(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
    }
}
