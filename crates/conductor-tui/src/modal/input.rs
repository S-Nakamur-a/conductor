//! モーダルの入力欄の描画。
//!
//! 折り返しを ratatui の Wrap に任せず自分で行に割るのは、カーソルの位置を
//! 描いた行と同じ計算で出すため。任せていた頃はカーソルが本文からずれた。

use conductor_core::text_input::TextInput;
use unicode_width::UnicodeWidthChar;

/// カーソルの居場所を示すブロック。端末のカーソルは 1 つしか置けないので、
/// モーダルは本文に混ぜて描く。
pub const CARET: char = '\u{2588}';

/// 本文を width 桁で折り返す。明示的な改行も尊重する。
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = vec![String::new()];
    let mut used = 0;
    for ch in text.chars() {
        if ch == '\n' {
            rows.push(String::new());
            used = 0;
            continue;
        }
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > width && !rows.last().expect("常に 1 行はある").is_empty() {
            rows.push(String::new());
            used = 0;
        }
        rows.last_mut().expect("常に 1 行はある").push(ch);
        used += w;
    }
    rows
}

/// カーソルを差し込んだ入力の表示行。
pub fn with_caret(input: &TextInput, width: usize) -> Vec<String> {
    wrap(
        &format!(
            "{}{CARET}{}",
            input.text_before_cursor(),
            input.text_after_cursor()
        ),
        width,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 明示的な改行は行になる() {
        assert_eq!(wrap("ab\ncd", 80), ["ab", "cd"]);
    }

    #[test]
    fn 長い行は幅で折り返す() {
        assert_eq!(wrap("0123456789", 4), ["0123", "4567", "89"]);
    }

    #[test]
    fn 全角文字は境界で割れない() {
        // CJK は 1 文字 2 桁。幅 3 では 1 行に 1 文字しか収まらない。
        assert_eq!(wrap("あい", 3), ["あ", "い"]);
    }

    #[test]
    fn 空の本文は1行になる() {
        assert_eq!(wrap("", 80), [""]);
    }

    #[test]
    fn カーソルはその位置の行に入る() {
        let mut input = TextInput::new_multiline();
        input.set_text("ab\ncd");
        input.move_left();
        assert_eq!(
            with_caret(&input, 80),
            ["ab".to_string(), format!("c{CARET}d")]
        );
    }
}
