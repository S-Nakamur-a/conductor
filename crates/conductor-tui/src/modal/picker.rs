//! 一覧から 1 つ選ぶモーダルが共有する部品。
//!
//! 絞り込みつきの一覧では素の印字可能キーは必ず入力に落ちる。j/k で下に動くと
//! 「j」がタイプできなくなるので、移動は矢印と修飾つきのキーだけが担う。

use conductor_core::keymap::{Action, KeyContext, KeyMap};
use conductor_core::text_input::TextInput;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// 一覧の選択位置。窓の高さは描画側が決めるので、ここは選択だけを持つ。
#[derive(Debug, Default)]
pub struct Cursor {
    pub selected: usize,
}

impl Cursor {
    /// keymap の Overlay 層で解決したナビゲーションを適用する。消費したら true。
    pub fn navigate(&mut self, keymap: &KeyMap, key: KeyEvent, len: usize) -> bool {
        let Some(action) = keymap.resolve(&key, KeyContext::Overlay) else {
            return false;
        };
        match action {
            Action::NavigateDown => {
                if self.selected + 1 < len {
                    self.selected += 1;
                }
            }
            Action::NavigateUp => self.selected = self.selected.saturating_sub(1),
            Action::GoToTop => self.selected = 0,
            Action::GoToBottom => self.selected = len.saturating_sub(1),
            _ => return false,
        }
        true
    }

    pub fn clamp(&mut self, len: usize) {
        self.selected = self.selected.min(len.saturating_sub(1));
    }
}

/// 素の印字可能文字 (Ctrl/Alt/Super なし)。Shift を除外しないのは、Shift+G が
/// タイプすべき文字 G そのものだから。
pub fn is_typing(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char(_))
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
}

/// 絞り込み欄つきの一覧のキー。移動したか入力が変わったかを返す。
pub enum Filtered {
    /// 何も起きなかった。呼び出し側が意味を決める。
    Ignored,
    Moved,
    /// 入力が変わった。呼び出し側は選択を戻したり検索をやり直したりする。
    Typed,
}

pub fn filtered_key(
    cursor: &mut Cursor,
    input: &mut TextInput,
    keymap: &KeyMap,
    key: KeyEvent,
    len: usize,
) -> Filtered {
    if !is_typing(key) && cursor.navigate(keymap, key, len) {
        return Filtered::Moved;
    }
    match key.code {
        KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
            input.delete_to_line_start();
            Filtered::Typed
        }
        KeyCode::Backspace | KeyCode::Delete | KeyCode::Char(_) => {
            input.handle_key(key);
            Filtered::Typed
        }
        _ => {
            input.handle_key(key);
            Filtered::Ignored
        }
    }
}

/// 絞り込み欄への貼り付け。絞り込みが変われば並びも変わるので選択を先頭へ戻す。
pub fn filtered_paste(cursor: &mut Cursor, input: &mut TextInput, text: &str) {
    input.insert_str(text);
    cursor.selected = 0;
}

/// 一覧の一部が窓に入るよう、先頭に落とす行数を決める。
pub fn scroll_for(selected: usize, len: usize, height: usize) -> usize {
    if height == 0 || len <= height {
        return 0;
    }
    selected.saturating_sub(height / 2).min(len - height)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keymap() -> KeyMap {
        KeyMap::with_warnings(&toml::Table::new()).0
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn 素の印字可能な文字は文字入力になる() {
        for c in ['j', 'k', 'g', 'G'] {
            assert!(is_typing(key(KeyCode::Char(c))), "{c}");
        }
        assert!(is_typing(KeyEvent::new(
            KeyCode::Char('G'),
            KeyModifiers::SHIFT
        )));
    }

    #[test]
    fn 修飾付きと名前付きのキーは文字入力ではない() {
        for k in [
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT),
            key(KeyCode::Up),
            key(KeyCode::Down),
        ] {
            assert!(!is_typing(k), "{k:?}");
        }
    }

    #[test]
    fn 絞り込み一覧では_jは移動せずタイプされる() {
        let (mut cursor, mut input) = (Cursor::default(), TextInput::new());
        let km = keymap();
        assert!(matches!(
            filtered_key(&mut cursor, &mut input, &km, key(KeyCode::Char('j')), 5),
            Filtered::Typed
        ));
        assert_eq!((cursor.selected, input.text()), (0, "j"));

        assert!(matches!(
            filtered_key(&mut cursor, &mut input, &km, key(KeyCode::Down), 5),
            Filtered::Moved
        ));
        assert_eq!((cursor.selected, input.text()), (1, "j"));
    }

    #[test]
    fn 移動は両端で止まる() {
        let mut cursor = Cursor::default();
        let km = keymap();
        cursor.navigate(&km, key(KeyCode::Up), 3);
        assert_eq!(cursor.selected, 0);
        for _ in 0..5 {
            cursor.navigate(&km, key(KeyCode::Down), 3);
        }
        assert_eq!(cursor.selected, 2);
        cursor.navigate(&km, key(KeyCode::Char('G')), 0);
        assert_eq!(cursor.selected, 0, "空の一覧で下端に飛んでも 0");
    }

    #[test]
    fn 窓は選択を真ん中に置き両端では詰める() {
        assert_eq!(scroll_for(0, 10, 4), 0);
        assert_eq!(scroll_for(5, 10, 4), 3);
        assert_eq!(scroll_for(9, 10, 4), 6);
        assert_eq!(scroll_for(2, 3, 10), 0, "窓より短ければ動かさない");
    }
}
