//! ダッシュボードのオーバーレイが共有するテキスト入力の描画ヘルパー:
//! カーソル位置の設定、ブロックカーソルの整形、カーソル追跡付きの複数行折り返し。

use crate::text_input::TextInput;
use ratatui::Frame;
use ratatui::layout::{Position, Rect};

/// 1行の TextInput 内のカーソル位置に合わせて、IME 用のターミナルカーソル位置を
/// 設定する。
pub(super) fn set_cursor_for_input(frame: &mut Frame, area: Rect, buffer: &TextInput) {
    let text_width = buffer.display_width_before_cursor() as u16;
    let cursor_x = area.x + text_width;
    let cursor_y = area.y;
    if cursor_x < area.x + area.width && cursor_y < area.y + area.height {
        frame.set_cursor_position(Position::new(cursor_x, cursor_y));
    }
}

/// カーソル位置にブロックカーソルを添えて、1行の TextInput を整形する。
pub(super) fn format_input_with_cursor(buffer: &TextInput) -> String {
    format!(
        "{}\u{2588}{}",
        buffer.text_before_cursor(),
        buffer.text_after_cursor()
    )
}

/// テキストを最大 width 表示桁の見た目上の行に折り返す。長い行はハード分割し
/// （明示的な \n も尊重する）。折り返し後の各行と、その中での cursor_char の
/// (row, col) を返す — 呼び出し元がカーソルを配置し、見える位置までスクロール
/// できるようにするため。これは実際に描画されるものと完全に一致する（ratatui
/// 自身の Wrap は使わず、この行をそのまま描く）ので、Paragraph が裏で勝手に
/// 再折り返しをしていた頃のようにカーソルがテキストからずれることはない。
pub(super) fn wrap_with_cursor(
    text: &str,
    width: usize,
    cursor_char: char,
) -> (Vec<String>, usize, usize) {
    use unicode_width::UnicodeWidthChar;
    let width = width.max(1);
    let mut rows: Vec<String> = vec![String::new()];
    let mut cur_w = 0usize;
    let mut cursor_pos = (0usize, 0usize);
    for ch in text.chars() {
        if ch == '\n' {
            rows.push(String::new());
            cur_w = 0;
            continue;
        }
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if cur_w + cw > width && !rows.last().unwrap().is_empty() {
            rows.push(String::new());
            cur_w = 0;
        }
        if ch == cursor_char {
            cursor_pos = (rows.len() - 1, cur_w);
        }
        rows.last_mut().unwrap().push(ch);
        cur_w += cw;
    }
    (rows, cursor_pos.0, cursor_pos.1)
}

#[cfg(test)]
mod tests {
    use super::wrap_with_cursor;

    #[test]
    fn 明示的な改行は行になる() {
        let (rows, r, c) = wrap_with_cursor("ab\ncd\u{2588}", 80, '\u{2588}');
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], "ab");
        // カーソルの記号は行1の "cd" の後ろに位置する。
        assert_eq!((r, c), (1, 2));
    }

    #[test]
    fn 長い行は幅で折り返しカーソルも追う() {
        // 10文字、幅4 → 4,4,2 の行になる。カーソルの記号は末尾にある。
        let (rows, r, c) = wrap_with_cursor("0123456789\u{2588}", 4, '\u{2588}');
        assert_eq!(rows, vec!["0123", "4567", "89\u{2588}"]);
        assert_eq!((r, c), (2, 2));
    }

    #[test]
    fn 全角文字は境界で割れない() {
        // CJK の各文字は幅2桁。幅3では1行につき1文字しか収まらない。
        let (rows, _r, _c) = wrap_with_cursor("あい", 3, '\u{2588}');
        assert_eq!(rows, vec!["あ", "い"]);
    }

    #[test]
    fn 空の本文は1行と原点のカーソルになる() {
        let (rows, r, c) = wrap_with_cursor("\u{2588}", 80, '\u{2588}');
        assert_eq!(rows.len(), 1);
        assert_eq!((r, c), (0, 0));
    }
}
