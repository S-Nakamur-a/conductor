//! Tests for `TextInput` construction, editing, and cursor movement.

use super::*;

#[test]
fn test_new_and_empty() {
    let ti = TextInput::new();
    assert!(ti.is_empty());
    assert_eq!(ti.text(), "");
    assert_eq!(ti.cursor, 0);
}

#[test]
fn test_insert_char() {
    let mut ti = TextInput::new();
    ti.insert_char('h');
    ti.insert_char('i');
    assert_eq!(ti.text(), "hi");
    assert_eq!(ti.cursor, 2);
}

#[test]
fn test_insert_at_cursor() {
    let mut ti = TextInput::new();
    ti.insert_char('a');
    ti.insert_char('c');
    ti.move_left();
    ti.insert_char('b');
    assert_eq!(ti.text(), "abc");
}

#[test]
fn test_delete_backward() {
    let mut ti = TextInput::new();
    ti.set_text("abc");
    ti.delete_backward();
    assert_eq!(ti.text(), "ab");
    ti.move_home();
    ti.delete_backward(); // no-op at start
    assert_eq!(ti.text(), "ab");
}

#[test]
fn test_delete_forward() {
    let mut ti = TextInput::new();
    ti.set_text("abc");
    ti.move_home();
    ti.delete_forward();
    assert_eq!(ti.text(), "bc");
    ti.move_end();
    ti.delete_forward(); // no-op at end
    assert_eq!(ti.text(), "bc");
}

#[test]
fn test_move_left_right() {
    let mut ti = TextInput::new();
    ti.set_text("abc");
    assert_eq!(ti.cursor, 3);
    ti.move_left();
    assert_eq!(ti.cursor, 2);
    ti.move_left();
    assert_eq!(ti.cursor, 1);
    ti.move_right();
    assert_eq!(ti.cursor, 2);
    // Move left past start
    ti.move_home();
    ti.move_left();
    assert_eq!(ti.cursor, 0);
    // Move right past end
    ti.move_end();
    ti.move_right();
    assert_eq!(ti.cursor, 3);
}

#[test]
fn test_move_home_end() {
    let mut ti = TextInput::new();
    ti.set_text("hello");
    ti.move_home();
    assert_eq!(ti.cursor, 0);
    ti.move_end();
    assert_eq!(ti.cursor, 5);
}

#[test]
fn test_multibyte_chars() {
    let mut ti = TextInput::new();
    ti.insert_char('あ');
    ti.insert_char('い');
    ti.insert_char('う');
    assert_eq!(ti.text(), "あいう");
    assert_eq!(ti.cursor, 9); // 3 bytes per char
    ti.move_left();
    assert_eq!(ti.cursor, 6);
    ti.delete_backward();
    assert_eq!(ti.text(), "あう");
    ti.delete_forward();
    assert_eq!(ti.text(), "あ");
}

#[test]
fn test_word_movement() {
    let mut ti = TextInput::new();
    ti.set_text("hello world foo");
    ti.move_home();
    ti.move_word_right();
    assert_eq!(ti.cursor, 6); // after "hello "
    ti.move_word_right();
    assert_eq!(ti.cursor, 12); // after "world "
    ti.move_word_left();
    assert_eq!(ti.cursor, 6); // back to "world"
    ti.move_word_left();
    assert_eq!(ti.cursor, 0); // back to start
}

#[test]
fn test_delete_to_line_start_single_line() {
    let mut ti = TextInput::new();
    ti.set_text("hello world");
    // Cursor at end: deletes everything
    ti.delete_to_line_start();
    assert_eq!(ti.text(), "");

    ti.set_text("hello world");
    ti.move_left(); // before 'd'
    ti.move_left(); // before 'l'
    ti.move_left(); // before 'r'
    ti.move_left(); // before 'o'
    ti.move_left(); // before 'w'
    ti.move_left(); // before ' '
    ti.delete_to_line_start();
    assert_eq!(ti.text(), " world");
    assert_eq!(ti.cursor, 0);
}

#[test]
fn test_delete_to_line_start_multiline() {
    let mut ti = TextInput::new_multiline();
    ti.set_text("line1\nline2\nline3");
    // Cursor at end of "line3"
    ti.delete_to_line_start();
    assert_eq!(ti.text(), "line1\nline2\n");

    // Cursor now at the empty third line; no-op since at line start
    ti.delete_to_line_start();
    assert_eq!(ti.text(), "line1\nline2\n");
}

#[test]
fn test_select_all_and_clear() {
    let mut ti = TextInput::new();
    ti.set_text("some text");
    ti.select_all_and_clear();
    assert!(ti.is_empty());
    assert_eq!(ti.cursor, 0);
}

#[test]
fn test_text_before_after_cursor() {
    let mut ti = TextInput::new();
    ti.set_text("abcdef");
    ti.move_home();
    ti.move_right();
    ti.move_right();
    ti.move_right();
    assert_eq!(ti.text_before_cursor(), "abc");
    assert_eq!(ti.text_after_cursor(), "def");
}

#[test]
fn test_multiline_cursor_row_col() {
    let mut ti = TextInput::new_multiline();
    ti.set_text("hello\nworld\nfoo");
    let (row, col) = ti.cursor_row_col();
    assert_eq!(row, 2);
    assert_eq!(col, 3); // "foo" width
}

#[test]
fn test_multiline_home_end() {
    let mut ti = TextInput::new_multiline();
    ti.set_text("line1\nline2\nline3");
    // Cursor at end of "line3"
    ti.move_home();
    assert_eq!(ti.text_before_cursor(), "line1\nline2\n");
    ti.move_end();
    assert_eq!(ti.text_after_cursor(), "");
}

#[test]
fn test_insert_str_single_line() {
    let mut ti = TextInput::new();
    ti.insert_str("hello\nworld");
    assert_eq!(ti.text(), "helloworld"); // newlines stripped
}

#[test]
fn test_insert_str_multiline() {
    let mut ti = TextInput::new_multiline();
    ti.insert_str("hello\nworld");
    assert_eq!(ti.text(), "hello\nworld");
}

#[test]
fn test_set_text_moves_cursor_to_end() {
    let mut ti = TextInput::new();
    ti.set_text("hello");
    assert_eq!(ti.cursor, 5);
    ti.set_text("hi");
    assert_eq!(ti.cursor, 2);
}

#[test]
fn test_display_width_japanese() {
    let mut ti = TextInput::new();
    ti.set_text("あいう");
    // Each Japanese char has display width 2, so total = 6
    assert_eq!(ti.display_width_before_cursor(), 6);
    ti.move_left();
    assert_eq!(ti.display_width_before_cursor(), 4);
}

#[test]
fn test_deref() {
    let ti = TextInput::new();
    assert!(ti.is_empty()); // str::is_empty via Deref
    let mut ti = TextInput::new();
    ti.set_text("Hello World");
    assert!(ti.contains("World")); // str::contains via Deref
    assert_eq!(ti.to_lowercase(), "hello world");
}
