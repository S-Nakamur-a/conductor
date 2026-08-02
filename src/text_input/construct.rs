//! TextInput の生成と読み取り専用の状態アクセサ。

use unicode_width::UnicodeWidthStr;

use super::TextInput;

impl TextInput {
    /// 新しい単一行のテキスト入力を作る。
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            cursor: 0,
            multiline: false,
        }
    }

    /// 新しい複数行のテキスト入力を作る。
    pub fn new_multiline() -> Self {
        Self {
            buffer: String::new(),
            cursor: 0,
            multiline: true,
        }
    }

    /// バッファをクリアし、カーソルを0に戻す。
    pub fn clear(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
    }

    /// バッファの内容を丸ごと置き換え、カーソルを末尾へ移動する。
    pub fn set_text(&mut self, text: &str) {
        self.buffer = text.to_string();
        self.cursor = self.buffer.len();
    }

    /// バッファ内容への参照を返す。
    pub fn text(&self) -> &str {
        &self.buffer
    }

    /// カーソルより前のテキストを返す。
    pub fn text_before_cursor(&self) -> &str {
        &self.buffer[..self.cursor]
    }

    /// カーソルより後のテキストを返す。
    pub fn text_after_cursor(&self) -> &str {
        &self.buffer[self.cursor..]
    }

    /// 複数行表示用に、カーソルの (row, col) を計算する。
    /// row と col は0始まり。col は表示幅（unicode 幅）で数える。
    pub fn cursor_row_col(&self) -> (usize, usize) {
        let before = self.text_before_cursor();
        let row = before.matches('\n').count();
        let last_line = before.rsplit('\n').next().unwrap_or(before);
        let col = UnicodeWidthStr::width(last_line);
        (row, col)
    }

    /// 現在行でカーソルより前にあるテキストの表示幅を返す。
    pub fn display_width_before_cursor(&self) -> usize {
        let before = self.text_before_cursor();
        let last_line = before.rsplit('\n').next().unwrap_or(before);
        UnicodeWidthStr::width(last_line)
    }
}
