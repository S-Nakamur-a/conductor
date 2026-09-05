use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::TextInput;

fn single(text: &str) -> TextInput {
    let mut input = TextInput::new();
    input.set_text(text);
    input
}

fn multi(text: &str) -> TextInput {
    let mut input = TextInput::new_multiline();
    input.set_text(text);
    input
}

fn left(input: &mut TextInput, n: usize) {
    (0..n).for_each(|_| input.move_left());
}

struct Case {
    name: &'static str,
    start: TextInput,
    ops: fn(&mut TextInput),
    text: &'static str,
    cursor: usize,
}

fn run(cases: Vec<Case>) {
    for case in cases {
        let mut input = case.start;
        (case.ops)(&mut input);
        assert_eq!(input.text(), case.text, "{}: text", case.name);
        assert_eq!(input.cursor, case.cursor, "{}: cursor", case.name);
    }
}

#[test]
fn 編集はカーソル位置に作用する() {
    run(vec![
        Case {
            name: "文字を入れるとカーソルが進む",
            start: single(""),
            ops: |i| {
                i.insert_char('h');
                i.insert_char('i');
            },
            text: "hi",
            cursor: 2,
        },
        Case {
            name: "挿入は末尾ではなくカーソル位置で起きる",
            start: single("ac"),
            ops: |i| {
                i.move_left();
                i.insert_char('b');
            },
            text: "abc",
            cursor: 2,
        },
        Case {
            name: "set_text はカーソルを末尾へ動かす",
            start: single("hello"),
            ops: |i| i.set_text("hi"),
            text: "hi",
            cursor: 2,
        },
        Case {
            name: "backspace は直前の文字を消す",
            start: single("abc"),
            ops: |i| i.delete_backward(),
            text: "ab",
            cursor: 2,
        },
        Case {
            name: "先頭での backspace は何もしない",
            start: single("ab"),
            ops: |i| {
                i.move_home();
                i.delete_backward();
            },
            text: "ab",
            cursor: 0,
        },
        Case {
            name: "delete は直後の文字を消す",
            start: single("abc"),
            ops: |i| {
                i.move_home();
                i.delete_forward();
            },
            text: "bc",
            cursor: 0,
        },
        Case {
            name: "末尾での delete は何もしない",
            start: single("bc"),
            ops: |i| i.delete_forward(),
            text: "bc",
            cursor: 2,
        },
        Case {
            name: "削除は文字単位で 3 バイトの文字を丸ごと消す",
            start: single("あいう"),
            ops: |i| {
                i.move_left();
                i.delete_backward();
                i.delete_forward();
            },
            text: "あ",
            cursor: 3,
        },
        Case {
            name: "行頭までの削除は末尾からなら全部消す",
            start: single("hello world"),
            ops: |i| i.delete_to_line_start(),
            text: "",
            cursor: 0,
        },
        Case {
            name: "行頭までの削除はカーソルより前だけ消す",
            start: single("hello world"),
            ops: |i| {
                left(i, 6);
                i.delete_to_line_start();
            },
            text: " world",
            cursor: 0,
        },
        Case {
            name: "行頭までの削除は改行で止まり、行頭では何もしない",
            start: multi("line1\nline2\nline3"),
            ops: |i| {
                i.delete_to_line_start();
                i.delete_to_line_start();
            },
            text: "line1\nline2\n",
            cursor: 12,
        },
        Case {
            name: "clear で入力が空になる",
            start: single("some text"),
            ops: |i| i.clear(),
            text: "",
            cursor: 0,
        },
        Case {
            name: "単一行の入力は貼り付けた改行を落とす",
            start: single(""),
            ops: |i| i.insert_str("hello\r\nworld"),
            text: "helloworld",
            cursor: 10,
        },
        Case {
            name: "複数行の入力は貼り付けた改行を残す",
            start: multi(""),
            ops: |i| i.insert_str("hello\nworld"),
            text: "hello\nworld",
            cursor: 11,
        },
    ]);
}

#[test]
fn 移動は文字境界と両端で止まる() {
    run(vec![
        Case {
            name: "左右は 1 文字ずつ動く",
            start: single("abc"),
            ops: |i| {
                i.move_left();
                i.move_left();
                i.move_right();
            },
            text: "abc",
            cursor: 2,
        },
        Case {
            name: "先頭を越える左移動は止まる",
            start: single("abc"),
            ops: |i| {
                i.move_home();
                i.move_left();
            },
            text: "abc",
            cursor: 0,
        },
        Case {
            name: "末尾を越える右移動は止まる",
            start: single("abc"),
            ops: |i| {
                i.move_end();
                i.move_right();
            },
            text: "abc",
            cursor: 3,
        },
        Case {
            name: "home と end は本文の両端へ飛ぶ",
            start: single("hello"),
            ops: |i| {
                i.move_home();
                i.move_end();
            },
            text: "hello",
            cursor: 5,
        },
        Case {
            name: "カーソルはバイトではなく文字単位で動く",
            start: single("あいう"),
            ops: |i| i.move_left(),
            text: "あいう",
            cursor: 6,
        },
        Case {
            name: "単語移動は右へ次の単語の先頭で止まる",
            start: single("hello world foo"),
            ops: |i| {
                i.move_home();
                i.move_word_right();
                i.move_word_right();
            },
            text: "hello world foo",
            cursor: 12,
        },
        Case {
            name: "単語移動は左へ今の単語の先頭で止まる",
            start: single("hello world foo"),
            ops: |i| {
                i.move_word_left();
                i.move_word_left();
            },
            text: "hello world foo",
            cursor: 6,
        },
        Case {
            name: "複数行の home と end は今の行の中に留まる",
            start: multi("line1\nline2\nline3"),
            ops: |i| {
                i.move_home();
                i.move_left();
                i.move_home();
                i.move_end();
            },
            text: "line1\nline2\nline3",
            cursor: 11,
        },
    ]);
}

#[test]
fn カーソル周りの本文と表示位置を答える() {
    let mut input = single("abcdef");
    input.move_home();
    (0..3).for_each(|_| input.move_right());
    assert_eq!(input.text_before_cursor(), "abc");
    assert_eq!(input.text_after_cursor(), "def");

    let input = multi("hello\nworld\nfoo");
    assert_eq!(input.cursor_row(), 2);
    assert_eq!(input.display_width_before_cursor(), 3);

    let mut input = single("あいう");
    assert_eq!(input.display_width_before_cursor(), 6, "全角は 2 桁");
    input.move_left();
    assert_eq!(input.display_width_before_cursor(), 4);
}

#[test]
fn キー入力は編集に変換される() {
    let key = |code, modifiers| KeyEvent::new(code, modifiers);
    let none = KeyModifiers::NONE;
    let cases: Vec<(&str, KeyEvent, TextInput, bool, &str, usize)> = vec![
        (
            "文字",
            key(KeyCode::Char('x'), none),
            single("ab"),
            true,
            "abx",
            3,
        ),
        (
            "Backspace",
            key(KeyCode::Backspace, none),
            single("ab"),
            true,
            "a",
            1,
        ),
        (
            "Delete",
            key(KeyCode::Delete, none),
            single("ab"),
            true,
            "ab",
            2,
        ),
        (
            "Left",
            key(KeyCode::Left, none),
            single("ab"),
            true,
            "ab",
            1,
        ),
        (
            "Home",
            key(KeyCode::Home, none),
            single("ab"),
            true,
            "ab",
            0,
        ),
        (
            "Ctrl+Left は単語移動",
            key(KeyCode::Left, KeyModifiers::CONTROL),
            single("hello world"),
            true,
            "hello world",
            6,
        ),
        (
            "Alt+Left も単語移動",
            key(KeyCode::Left, KeyModifiers::ALT),
            single("hello world"),
            true,
            "hello world",
            6,
        ),
        (
            "Ctrl+A はクリア",
            key(KeyCode::Char('a'), KeyModifiers::CONTROL),
            single("ab"),
            true,
            "",
            0,
        ),
        (
            "Enter は消費しない",
            key(KeyCode::Enter, none),
            single("ab"),
            false,
            "ab",
            2,
        ),
        (
            "Esc は消費しない",
            key(KeyCode::Esc, none),
            single("ab"),
            false,
            "ab",
            2,
        ),
    ];
    for (name, event, mut input, consumed, text, cursor) in cases {
        assert_eq!(input.handle_key(event), consumed, "{name}: consumed");
        assert_eq!(input.text(), text, "{name}: text");
        assert_eq!(input.cursor, cursor, "{name}: cursor");
    }
}

#[test]
fn 入力はstrへderefできる() {
    assert!(TextInput::new().is_empty());
    let input = single("Hello World");
    assert!(input.contains("World"));
    assert_eq!(input.to_lowercase(), "hello world");
    assert_eq!(input.to_string(), "Hello World");
}
