//! カーソル付きのテキスト入力バッファ。
//!
//! カーソルは本文へのバイトオフセットで、常に文字境界にある。単一行の入力は
//! 貼り付けた改行を落とす点だけが複数行と違う。クリップボードは持たず、
//! 貼り付けは呼び出し側が insert_str に渡す。

use std::fmt;
use std::ops::Deref;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthStr;

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Default)]
pub struct TextInput {
    buffer: String,
    cursor: usize,
    multiline: bool,
}

impl TextInput {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn new_multiline() -> Self {
        Self {
            multiline: true,
            ..Self::default()
        }
    }

    pub fn text(&self) -> &str {
        &self.buffer
    }

    pub fn text_before_cursor(&self) -> &str {
        &self.buffer[..self.cursor]
    }

    pub fn text_after_cursor(&self) -> &str {
        &self.buffer[self.cursor..]
    }

    /// カーソルのある行番号 (0 始まり)。
    pub fn cursor_row(&self) -> usize {
        self.text_before_cursor().matches('\n').count()
    }

    /// 現在行でカーソルより前にある本文の表示幅。全角は 2 桁。
    pub fn display_width_before_cursor(&self) -> usize {
        self.buffer[self.line_start()..self.cursor].width()
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
    }

    /// 本文を置き換え、カーソルを末尾に置く。
    pub fn set_text(&mut self, text: &str) {
        self.buffer = text.to_string();
        self.cursor = self.buffer.len();
    }

    pub fn insert_char(&mut self, c: char) {
        self.buffer.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    /// カーソル位置に文字列を挿入する。単一行の入力では改行を落とす。
    pub fn insert_str(&mut self, s: &str) {
        if self.multiline {
            self.buffer.insert_str(self.cursor, s);
            self.cursor += s.len();
        } else {
            let single_line: String = s.chars().filter(|c| !matches!(c, '\n' | '\r')).collect();
            self.buffer.insert_str(self.cursor, &single_line);
            self.cursor += single_line.len();
        }
    }

    pub fn delete_backward(&mut self) {
        let prev = self.prev_char_boundary();
        self.buffer.replace_range(prev..self.cursor, "");
        self.cursor = prev;
    }

    pub fn delete_forward(&mut self) {
        let next = self.next_char_boundary();
        self.buffer.replace_range(self.cursor..next, "");
    }

    pub fn delete_to_line_start(&mut self) {
        let line_start = self.line_start();
        self.buffer.replace_range(line_start..self.cursor, "");
        self.cursor = line_start;
    }

    pub fn move_left(&mut self) {
        self.cursor = self.prev_char_boundary();
    }

    pub fn move_right(&mut self) {
        self.cursor = self.next_char_boundary();
    }

    pub fn move_home(&mut self) {
        self.cursor = self.line_start();
    }

    pub fn move_end(&mut self) {
        self.cursor = self.line_end();
    }

    /// 直前の単語の先頭へ移動する。単語は ASCII 英数字の並び。
    pub fn move_word_left(&mut self) {
        let before = self
            .text_before_cursor()
            .trim_end_matches(|c: char| !c.is_ascii_alphanumeric())
            .trim_end_matches(|c: char| c.is_ascii_alphanumeric());
        self.cursor = before.len();
    }

    /// 次の単語の先頭へ移動する。単語は ASCII 英数字の並び。
    pub fn move_word_right(&mut self) {
        let after = self
            .text_after_cursor()
            .trim_start_matches(|c: char| c.is_ascii_alphanumeric())
            .trim_start_matches(|c: char| !c.is_ascii_alphanumeric());
        self.cursor = self.buffer.len() - after.len();
    }

    /// 文字入力・カーソル移動・削除・Ctrl+A のクリアを処理し、消費したら true。
    /// 貼り付けは呼び出し側がクリップボードを読んで insert_str に渡す。
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        let by_word = key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        match key.code {
            KeyCode::Backspace => self.delete_backward(),
            KeyCode::Delete => self.delete_forward(),
            KeyCode::Left if by_word => self.move_word_left(),
            KeyCode::Right if by_word => self.move_word_right(),
            KeyCode::Left => self.move_left(),
            KeyCode::Right => self.move_right(),
            KeyCode::Home => self.move_home(),
            KeyCode::End => self.move_end(),
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => self.clear(),
            KeyCode::Char(c) => self.insert_char(c),
            _ => return false,
        }
        true
    }

    fn line_start(&self) -> usize {
        self.text_before_cursor().rfind('\n').map_or(0, |nl| nl + 1)
    }

    fn line_end(&self) -> usize {
        self.text_after_cursor()
            .find('\n')
            .map_or(self.buffer.len(), |nl| self.cursor + nl)
    }

    fn prev_char_boundary(&self) -> usize {
        self.text_before_cursor()
            .chars()
            .next_back()
            .map_or(0, |c| self.cursor - c.len_utf8())
    }

    fn next_char_boundary(&self) -> usize {
        self.text_after_cursor()
            .chars()
            .next()
            .map_or(self.cursor, |c| self.cursor + c.len_utf8())
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
