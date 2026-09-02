//! KeyEvent を、本物の端末が子プロセスへ送るバイト列にする。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// 送るバイトを持たないキー (修飾キー単独など) は None。
pub fn key_to_bytes(key: &KeyEvent, app_cursor: bool) -> Option<Vec<u8>> {
    let mods = key.modifiers;
    Some(match key.code {
        KeyCode::Char(c) => char_bytes(c, mods),
        KeyCode::Enter => {
            if mods.contains(KeyModifiers::SHIFT) {
                // Claude Code に改行として扱わせるための CSI u。
                b"\x1b[13;2u".to_vec()
            } else if mods.contains(KeyModifiers::ALT) {
                vec![0x1b, b'\r']
            } else {
                vec![b'\r']
            }
        }
        KeyCode::Tab if mods.contains(KeyModifiers::SHIFT) => b"\x1b[Z".to_vec(),
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Backspace => match mods {
            m if m.contains(KeyModifiers::SUPER) => vec![0x15],
            m if m.contains(KeyModifiers::ALT) => vec![0x1b, 0x7f],
            m if m.contains(KeyModifiers::CONTROL) => vec![0x17],
            _ => vec![0x7f],
        },
        KeyCode::Delete => tilde_bytes(3, mods),
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => arrow_bytes(b'A', mods, app_cursor),
        KeyCode::Down => arrow_bytes(b'B', mods, app_cursor),
        KeyCode::Right => arrow_bytes(b'C', mods, app_cursor),
        KeyCode::Left => arrow_bytes(b'D', mods, app_cursor),
        KeyCode::Home => home_end_bytes(b'H', mods, app_cursor),
        KeyCode::End => home_end_bytes(b'F', mods, app_cursor),
        KeyCode::PageUp => tilde_bytes(5, mods),
        KeyCode::PageDown => tilde_bytes(6, mods),
        KeyCode::Insert => tilde_bytes(2, mods),
        KeyCode::F(n) => function_bytes(n, mods),
        _ => return None,
    })
}

fn utf8(c: char) -> Vec<u8> {
    let mut buf = [0u8; 4];
    c.encode_utf8(&mut buf).as_bytes().to_vec()
}

/// 強化キーボードプロトコルは大文字を「小文字 + SHIFT」で送ってくることがある。
fn shifted(c: char, mods: KeyModifiers) -> char {
    if mods.contains(KeyModifiers::SHIFT) {
        c.to_ascii_uppercase()
    } else {
        c
    }
}

fn char_bytes(c: char, mods: KeyModifiers) -> Vec<u8> {
    if mods.contains(KeyModifiers::CONTROL) {
        if c.is_ascii_alphabetic() {
            let ctrl = (c.to_ascii_lowercase() as u8) - b'a' + 1;
            return if mods.contains(KeyModifiers::ALT) {
                vec![0x1b, ctrl]
            } else {
                vec![ctrl]
            };
        }
        return match c {
            '[' | '3' => vec![0x1b],
            '\\' | '4' => vec![0x1c],
            ']' | '5' => vec![0x1d],
            '^' | '6' => vec![0x1e],
            '_' | '7' => vec![0x1f],
            '@' | '2' => vec![0x00],
            '/' | '8' => vec![0x7f],
            _ => utf8(c),
        };
    }
    if mods.contains(KeyModifiers::ALT) {
        let mut bytes = vec![0x1b];
        bytes.extend(utf8(shifted(c, mods)));
        return bytes;
    }
    utf8(shifted(c, mods))
}

/// xterm の符号化。修飾なしが 1 で、Shift 1 / Alt 2 / Ctrl 4 を足す。
/// Super はキーごとに意味が違うので、ここには入れない。
fn modifier_param(mods: KeyModifiers) -> u8 {
    let mut param = 1;
    if mods.contains(KeyModifiers::SHIFT) {
        param += 1;
    }
    if mods.contains(KeyModifiers::ALT) {
        param += 2;
    }
    if mods.contains(KeyModifiers::CONTROL) {
        param += 4;
    }
    param
}

/// DECCKM が効くのは修飾なしのときだけ。修飾があれば xterm はモードに関わらず CSI。
fn arrow_bytes(dir: u8, mods: KeyModifiers, app_cursor: bool) -> Vec<u8> {
    if mods.contains(KeyModifiers::SUPER) {
        return match dir {
            b'D' => b"\x1b[H".to_vec(),
            b'C' => b"\x1b[F".to_vec(),
            b'A' => b"\x1b[5~".to_vec(),
            _ => b"\x1b[6~".to_vec(),
        };
    }
    match modifier_param(mods) {
        1 if app_cursor => vec![0x1b, b'O', dir],
        1 => vec![0x1b, b'[', dir],
        param => format!("\x1b[1;{param}{}", dir as char).into_bytes(),
    }
}

fn home_end_bytes(code: u8, mods: KeyModifiers, app_cursor: bool) -> Vec<u8> {
    match modifier_param(mods) {
        1 if app_cursor => vec![0x1b, b'O', code],
        1 => vec![0x1b, b'[', code],
        param => format!("\x1b[1;{param}{}", code as char).into_bytes(),
    }
}

fn tilde_bytes(num: u8, mods: KeyModifiers) -> Vec<u8> {
    // Alt+Delete は readline の慣習で単語単位の前方削除。
    if num == 3 && mods.contains(KeyModifiers::ALT) && !mods.contains(KeyModifiers::CONTROL) {
        return vec![0x1b, b'd'];
    }
    match modifier_param(mods) {
        1 => format!("\x1b[{num}~").into_bytes(),
        param => format!("\x1b[{num};{param}~").into_bytes(),
    }
}

fn function_bytes(n: u8, mods: KeyModifiers) -> Vec<u8> {
    let (prefix, code) = match n {
        1 => ("O", 'P'),
        2 => ("O", 'Q'),
        3 => ("O", 'R'),
        4 => ("O", 'S'),
        5 => ("[15", '~'),
        6 => ("[17", '~'),
        7 => ("[18", '~'),
        8 => ("[19", '~'),
        9 => ("[20", '~'),
        10 => ("[21", '~'),
        11 => ("[23", '~'),
        12 => ("[24", '~'),
        _ => return Vec::new(),
    };
    match (modifier_param(mods), code) {
        (1, _) => format!("\x1b{prefix}{code}").into_bytes(),
        (param, '~') => format!("\x1b{prefix};{param}~").into_bytes(),
        (param, code) => format!("\x1b[1;{param}{code}").into_bytes(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(code: KeyCode, mods: KeyModifiers, app_cursor: bool) -> Vec<u8> {
        key_to_bytes(&KeyEvent::new(code, mods), app_cursor).unwrap()
    }

    #[test]
    fn 矢印とhome_endはdecckmと修飾に従う() {
        use KeyCode::{Down, End, Home, Left, Right, Up};
        const NONE: KeyModifiers = KeyModifiers::NONE;
        let cases: [(KeyCode, KeyModifiers, bool, &[u8]); 10] = [
            (Up, NONE, false, b"\x1b[A"),
            (Down, NONE, false, b"\x1b[B"),
            (Right, NONE, false, b"\x1b[C"),
            (Left, NONE, false, b"\x1b[D"),
            (Up, NONE, true, b"\x1bOA"),
            (Home, NONE, false, b"\x1b[H"),
            (End, NONE, true, b"\x1bOF"),
            // 修飾が付くとモードに関わらず CSI の 1;<param> 形式になる。
            (Up, KeyModifiers::SHIFT, true, b"\x1b[1;2A"),
            (Up, KeyModifiers::SHIFT, false, b"\x1b[1;2A"),
            (Left, KeyModifiers::SUPER, false, b"\x1b[H"),
        ];
        for (code, mods, app_cursor, expected) in cases {
            assert_eq!(bytes(code, mods, app_cursor), expected, "{code:?} {mods:?}");
        }
    }

    #[test]
    fn 制御文字と修飾付きの文字() {
        const NONE: KeyModifiers = KeyModifiers::NONE;
        let cases: [(char, KeyModifiers, &[u8]); 6] = [
            ('a', NONE, b"a"),
            ('a', KeyModifiers::SHIFT, b"A"),
            ('c', KeyModifiers::CONTROL, &[0x03]),
            ('[', KeyModifiers::CONTROL, &[0x1b]),
            ('c', KeyModifiers::ALT, b"\x1bc"),
            (
                'c',
                KeyModifiers::CONTROL | KeyModifiers::ALT,
                &[0x1b, 0x03],
            ),
        ];
        for (c, mods, expected) in cases {
            assert_eq!(
                bytes(KeyCode::Char(c), mods, false),
                expected,
                "{c} {mods:?}"
            );
        }
    }

    #[test]
    fn 意味のないキーはバイトを持たない() {
        assert!(key_to_bytes(&KeyEvent::new(KeyCode::Null, KeyModifiers::NONE), false).is_none());
    }
}
