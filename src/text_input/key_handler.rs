//! Unified key event handler for `TextInput`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::TextInput;

impl TextInput {
    /// Handle common editing key events (character input, cursor movement,
    /// deletion, word movement, select-all-clear).
    ///
    /// Returns `true` if the key was consumed by this handler.
    /// Clipboard paste (Ctrl+V) and Cmd+Backspace (clear) are **not** handled
    /// here because they require external clipboard/app state — callers should
    /// handle those before delegating to this method.
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
