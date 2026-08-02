//! TextInput の統一されたキーイベントハンドラ。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::TextInput;

impl TextInput {
    /// 一般的な編集用キーイベント（文字入力、カーソル移動、削除、単語移動、
    /// 全選択クリア）を処理する。
    ///
    /// このハンドラがキーを消費した場合は true を返す。
    /// クリップボード貼り付け（Ctrl+V）と Cmd+Backspace（クリア）はここでは扱わない。
    /// 外部のクリップボードやアプリ状態が必要なため、呼び出し側がこのメソッドに
    /// 委譲する前に処理すること。
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Backspace => {
                self.delete_backward();
                true
            }
            KeyCode::Delete => {
                self.delete_forward();
                true
            }
            KeyCode::Left
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    || key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.move_word_left();
                true
            }
            KeyCode::Right
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    || key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.move_word_right();
                true
            }
            KeyCode::Left => {
                self.move_left();
                true
            }
            KeyCode::Right => {
                self.move_right();
                true
            }
            KeyCode::Home => {
                self.move_home();
                true
            }
            KeyCode::End => {
                self.move_end();
                true
            }
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.select_all_and_clear();
                true
            }
            KeyCode::Char(c) => {
                self.insert_char(c);
                true
            }
            _ => false,
        }
    }
}
