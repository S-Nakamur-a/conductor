//! 実 PTY を起動せずに固定できる事実だけを試す: 入力の符号化とサニタイズ、
//! 生履歴のトリムと再生、UTF-8 の分割、ロケール判定。

use std::collections::VecDeque;

use super::io::{encode_mouse_wheel, sanitize_pasted_text, scroll_arrow_sequence};
use super::locale::{utf8_chunks, utf8_locale_overrides};
use super::reader::trim_raw_history;
use super::screen::rebuild_parser;

#[test]
fn 矢印の列はdecckmと向きに従う() {
    // アプリケーションカーソルキーモードは SS3 (ESC O)、通常は CSI (ESC [)。
    let cases = [
        (true, true, &b"\x1bOA"[..]),
        (false, true, &b"\x1bOB"[..]),
        (true, false, &b"\x1b[A"[..]),
        (false, false, &b"\x1b[B"[..]),
    ];
    for (up, app_cursor, want) in cases {
        assert_eq!(
            scroll_arrow_sequence(up, app_cursor),
            want,
            "{up} {app_cursor}"
        );
    }
}

#[test]
fn 貼り付けは本文だけ残す() {
    let cases = [
        // 素のテキスト、タブ、改行、マルチバイトはそのまま。
        (
            "echo hello\tworld\nsecond line\n",
            "echo hello\tworld\nsecond line\n",
        ),
        ("こんにちは\tworld\n", "こんにちは\tworld\n"),
        // CR と CRLF は LF に正規化する。
        ("a\r\nb\rc", "a\nb\nc"),
        // CSI (SGR カラー) は丸ごと落ちる。
        ("\x1b[31mred\x1b[0m text", "red text"),
        // BEL 終端の OSC (ウィンドウタイトル) も丸ごと落ちる。
        ("before\x1b]0;evil title\x07after", "beforeafter"),
        // 裸の制御バイト (NUL / BEL / BS / DEL) は消え、本文は無傷。
        ("a\x00b\x07c\x08d\x7fe", "abcde"),
    ];
    for (input, want) in cases {
        assert_eq!(sanitize_pasted_text(input), want, "{input:?}");
    }
}

/// 挙動を左右する重要ケース: paste-end マーカーの後にコマンドを忍ばせた
/// クリップボードが bracketed paste から抜け出せてはならない。
#[test]
fn 紛れ込んだ貼り付け終了マーカーを取り除く() {
    let out = sanitize_pasted_text("safe\x1b[201~rm -rf /\n");
    assert_eq!(out, "saferm -rf /\n");
    assert!(!out.contains("201~"));
    assert!(!out.contains('\x1b'));
}

#[test]
fn ホイールの符号化は方式と向きに従う() {
    use vt100::MouseProtocolEncoding::{Default, Sgr};
    let cases = [
        // SGR: アップ = ボタン 64、ダウン = 65。
        (true, 5, 9, Sgr, b"\x1b[<64;5;9M".to_vec()),
        (false, 1, 1, Sgr, b"\x1b[<65;1;1M".to_vec()),
        // 座標 0 は 1 にクランプして well-formed に保つ。
        (true, 0, 0, Sgr, b"\x1b[<64;1;1M".to_vec()),
        // レガシー X10 は各値を 32 ずらす (64+32 = 96)。
        (true, 2, 3, Default, vec![0x1b, b'[', b'M', 96, 34, 35]),
    ];
    for (up, col, row, encoding, want) in cases {
        assert_eq!(
            encode_mouse_wheel(up, col, row, encoding),
            want,
            "{up} {col} {row}"
        );
    }
}

#[test]
fn 履歴は上限以下かつ行境界に揃える() {
    let mut history: VecDeque<u8> = b"aaaa\nbbbb\ncccc\ndddd\n".iter().copied().collect();
    trim_raw_history(&mut history, 10);

    let text = String::from_utf8(history.into_iter().collect()).unwrap();
    assert!(text.len() <= 10, "{text:?}");
    assert!(!text.contains("aaaa"), "{text:?}");
    // 残った完全な行はどれも元のまま (先頭に欠けた行が残っていない)。
    for line in text.split_inclusive('\n') {
        if line.ends_with('\n') {
            assert!(["bbbb\n", "cccc\n", "dddd\n"].contains(&line), "{line:?}");
        }
    }
}

#[test]
fn 上限以下の履歴には触らない() {
    for (bytes, cap) in [(&b"hello\n"[..], 1024), (&b"abcd\n"[..], 5), (&b""[..], 0)] {
        let mut history: VecDeque<u8> = bytes.iter().copied().collect();
        trim_raw_history(&mut history, cap);
        assert_eq!(history.into_iter().collect::<Vec<u8>>(), bytes, "cap={cap}");
    }
}

/// 改行を一切含まない超過バッファを空まで削ってはならない。リサイズで画面が真っ白に
/// なる方が、先頭行が欠けるよりはるかに悪い。エスケープだらけの TUI 出力で起きる。
#[test]
fn 改行が無くても中身は空にしない() {
    let mut history: VecDeque<u8> = std::iter::repeat_n(b'x', 100).collect();
    trim_raw_history(&mut history, 10);
    assert!(!history.is_empty(), "history was drained to empty");
    assert!(history.len() <= 10);
    assert!(history.iter().all(|&b| b == b'x'));
}

/// 再生が新しい幅で折り返し直すことが、リサイズ経路の核心。vt100 自身の set_size は
/// リフローしない — その事実もここで固定しておく。
#[test]
fn 再生は新しい幅で折り返し直す() {
    // 明示的な改行の無い 12 文字の論理行。
    let stream = b"ABCDEFGHIJKL";

    let mut narrow = vt100::Parser::new(10, 4, 100);
    narrow.process(stream);
    assert_eq!(narrow.screen().contents().trim_end(), "ABCDEFGHIJKL");

    narrow.set_size(10, 12);
    assert_eq!(
        narrow.screen().contents().trim_end(),
        "ABCD\nEFGH\nIJKL",
        "set_size unexpectedly reflowed — the bug may be fixed upstream",
    );

    let history: VecDeque<u8> = stream.iter().copied().collect();
    let wide = rebuild_parser(&history, 10, 12, 100);
    assert_eq!(wide.screen().contents().trim_end(), "ABCDEFGHIJKL");
}

#[test]
fn 空の履歴でもパーサを組み直せる() {
    let parser = rebuild_parser(&VecDeque::new(), 5, 20, 100);
    assert_eq!(parser.screen().contents().trim_end(), "");
}

/// オルタネート画面の出入りはストリーム中の純粋なバイト列なので、再生すれば
/// 正しい最終状態 (通常画面) が組み上がる。
#[test]
fn 代替画面の往復も組み直せる() {
    let mut stream: Vec<u8> = Vec::new();
    stream.extend_from_slice(b"normal-before\r\n");
    stream.extend_from_slice(b"\x1b[?1049h");
    stream.extend_from_slice(b"ALT-CONTENT");
    stream.extend_from_slice(b"\x1b[?1049l");
    stream.extend_from_slice(b"normal-after");

    let parser = rebuild_parser(&stream.into_iter().collect(), 6, 40, 100);
    assert!(!parser.screen().alternate_screen());
    let contents = parser.screen().contents();
    assert!(contents.contains("normal-before"), "got: {contents:?}");
    assert!(contents.contains("normal-after"), "got: {contents:?}");
    // オルタネート画面の中身は通常のグリッドに漏れない。
    assert!(!contents.contains("ALT-CONTENT"), "got: {contents:?}");
}

#[test]
fn 分割はマルチバイト文字を割らない() {
    // 単純なバイト分割なら文字の途中に着地する組み合わせを並べる。
    for (text, max) in [
        ("あいうえお", 4),
        ("abc日本語def", 5),
        ("あ", 1),
        ("", 1024),
    ] {
        let chunks = utf8_chunks(text, max);
        assert_eq!(chunks.concat(), text, "{text:?} max={max}");
        for chunk in &chunks {
            assert!(!chunk.is_empty(), "{text:?} max={max}");
            // max を超えられるのは、1 文字がそれより大きいときだけ。
            assert!(
                chunk.len() <= max || chunk.chars().count() == 1,
                "{chunk:?} max={max}"
            );
        }
    }
}

#[test]
fn ロケール判定はposixの優先順位に従う() {
    const UTF8: (&str, &str) = ("LC_CTYPE", "C.UTF-8");
    type Env = Option<&'static str>;
    type Overrides = (Vec<(&'static str, &'static str)>, Vec<&'static str>);
    // (LC_ALL, LC_CTYPE, LANG) → (設定する, 削除する)
    let cases: [(Env, Env, Env, Overrides); 7] = [
        // 何も無ければ vim が latin1 に落ちるので UTF-8 を注入する。
        (None, None, None, (vec![UTF8], vec![])),
        // macOS では空のままなことが多い。空は未設定と同じで、非 UTF-8 ではない。
        (Some(""), Some(""), Some(""), (vec![UTF8], vec![])),
        // すでに UTF-8 ならユーザの設定を尊重する (綴りと大小文字は問わない)。
        (None, Some("UTF-8"), None, (vec![], vec![])),
        (None, None, Some("en_US.UTF-8"), (vec![], vec![])),
        (None, None, Some("ja_JP.utf8"), (vec![], vec![])),
        // LC_ALL が LANG より優先される。
        (Some("C.UTF-8"), None, Some("C"), (vec![], vec![])),
        // 非 UTF-8 の LC_ALL は注入した LC_CTYPE を覆い隠すので消す。
        (
            Some("C"),
            Some("C"),
            Some("C"),
            (vec![UTF8], vec!["LC_ALL"]),
        ),
    ];
    for (lc_all, lc_ctype, lang, want) in cases {
        assert_eq!(
            utf8_locale_overrides(lc_all, lc_ctype, lang),
            want,
            "{lc_all:?} {lc_ctype:?} {lang:?}"
        );
    }
}

/// LC_ALL が無ければ、LANG が非 UTF-8 でも LC_ALL に触らない。
#[test]
fn 無いlc_allは削除対象にしない() {
    let (sets, removes) = utf8_locale_overrides(None, None, Some("C"));
    assert_eq!(sets, vec![("LC_CTYPE", "C.UTF-8")]);
    assert!(removes.is_empty());
}
