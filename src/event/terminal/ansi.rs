//! KeyEvent から PTY へ転送する ANSI バイト列への変換。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// crossterm の KeyEvent を、実際のターミナルが子プロセスに送るであろう
/// ANSI バイト列に変換する。
///
/// 意味のあるバイト表現を持たないキーイベント（Shift 単独押下など）には
/// None を返す。
pub(super) fn key_event_to_ansi(key: &KeyEvent, app_cursor: bool) -> Option<Vec<u8>> {
    let mods = key.modifiers;
    let data = match key.code {
        // 文字キー
        KeyCode::Char(c) => char_with_modifiers(c, mods),

        // エンター / タブ
        KeyCode::Enter => {
            if mods.contains(KeyModifiers::SHIFT) {
                // Shift+Enter → CSI u とすることで Claude Code に改行として扱わせる。
                b"\x1b[13;2u".to_vec()
            } else if mods.contains(KeyModifiers::ALT) {
                vec![0x1b, b'\r']
            } else {
                vec![b'\r']
            }
        }
        KeyCode::Tab => {
            if mods.contains(KeyModifiers::SHIFT) {
                b"\x1b[Z".to_vec()
            } else {
                vec![b'\t']
            }
        }
        KeyCode::BackTab => b"\x1b[Z".to_vec(),

        // バックスペース / デリート
        KeyCode::Backspace => {
            if mods.contains(KeyModifiers::SUPER) {
                // Cmd+Backspace → 行頭まで削除（Ctrl+U 相当）。
                vec![0x15]
            } else if mods.contains(KeyModifiers::ALT) {
                // Option+Backspace → 単語単位で後方削除（ESC DEL）。
                vec![0x1b, 0x7f]
            } else if mods.contains(KeyModifiers::CONTROL) {
                // Ctrl+Backspace → 単語単位で後方削除（Ctrl+W と同じ）。
                vec![0x17]
            } else {
                vec![0x7f]
            }
        }
        KeyCode::Delete => tilde_key_with_modifiers(3, &mods),

        // エスケープ
        KeyCode::Esc => vec![0x1b],

        // 矢印キー
        KeyCode::Up => arrow_with_modifiers(b'A', &mods, app_cursor),
        KeyCode::Down => arrow_with_modifiers(b'B', &mods, app_cursor),
        KeyCode::Right => arrow_with_modifiers(b'C', &mods, app_cursor),
        KeyCode::Left => arrow_with_modifiers(b'D', &mods, app_cursor),

        // ホーム / エンド
        // DECCKM は無修飾の場合にのみ適用される: アプリケーションカーソルキー
        // モードが有効なら SS3（ESC O H）、そうでなければ CSI を使う。
        KeyCode::Home => {
            let p = xterm_modifier_param(&mods);
            if p == 1 {
                if app_cursor {
                    b"\x1bOH".to_vec()
                } else {
                    b"\x1b[H".to_vec()
                }
            } else {
                format!("\x1b[1;{p}H").into_bytes()
            }
        }
        KeyCode::End => {
            let p = xterm_modifier_param(&mods);
            if p == 1 {
                if app_cursor {
                    b"\x1bOF".to_vec()
                } else {
                    b"\x1b[F".to_vec()
                }
            } else {
                format!("\x1b[1;{p}F").into_bytes()
            }
        }

        // ページアップ / ページダウン
        KeyCode::PageUp => tilde_key_with_modifiers(5, &mods),
        KeyCode::PageDown => tilde_key_with_modifiers(6, &mods),

        // インサート
        KeyCode::Insert => tilde_key_with_modifiers(2, &mods),

        // ファンクションキー
        KeyCode::F(n) => f_key_to_ansi(n, &mods),

        // 修飾キー単独、または未知のキー — 送るバイトなし
        _ => return None,
    };

    Some(data)
}

/// 修飾キー付きの文字キーをバイト列に変換する。
fn char_with_modifiers(c: char, mods: KeyModifiers) -> Vec<u8> {
    if mods.contains(KeyModifiers::CONTROL) {
        if c.is_ascii_lowercase() || c.is_ascii_uppercase() {
            // Ctrl+文字 → 制御バイト（Ctrl+A = 0x01, ..., Ctrl+Z = 0x1a）。
            let ctrl_byte = (c.to_ascii_lowercase() as u8)
                .wrapping_sub(b'a')
                .wrapping_add(1);
            if mods.contains(KeyModifiers::ALT) {
                vec![0x1b, ctrl_byte]
            } else {
                vec![ctrl_byte]
            }
        } else {
            // Ctrl + 非文字キー（例: Ctrl+[ = ESC, Ctrl+] = 0x1d）。
            match c {
                '[' | '3' => vec![0x1b],
                '\\' | '4' => vec![0x1c],
                ']' | '5' => vec![0x1d],
                '^' | '6' => vec![0x1e],
                '_' | '7' => vec![0x1f],
                '@' | '2' => vec![0x00],
                '/' | '8' => vec![0x7f],
                _ => {
                    let mut buf = [0u8; 4];
                    let s = c.encode_utf8(&mut buf);
                    s.as_bytes().to_vec()
                }
            }
        }
    } else if mods.contains(KeyModifiers::ALT) {
        // Alt+文字 → ESC プレフィックス + 文字（メタキーエンコーディング）。
        let ch = if mods.contains(KeyModifiers::SHIFT) && c.is_ascii_lowercase() {
            c.to_ascii_uppercase()
        } else {
            c
        };
        let mut buf = vec![0x1b];
        let mut char_buf = [0u8; 4];
        let s = ch.encode_utf8(&mut char_buf);
        buf.extend_from_slice(s.as_bytes());
        buf
    } else {
        // 通常の文字、または Shift+文字（enhanced keyboard protocol が小文字 +
        // SHIFT 修飾子として送ってくることがあるため、ここで手動で shift を適用する）。
        let ch = if mods.contains(KeyModifiers::SHIFT) && c.is_ascii_lowercase() {
            c.to_ascii_uppercase()
        } else {
            c
        };
        let mut buf = [0u8; 4];
        let s = ch.encode_utf8(&mut buf);
        s.as_bytes().to_vec()
    }
}

/// crossterm の修飾子から xterm 用の modifier パラメータを計算する。
///
/// xterm は修飾子を 1 + bitmask としてエンコードする:
///   Shift = 1, Alt = 2, Ctrl = 4, Super/Meta = 8。
/// 修飾子が何もなければ 1 を返す。
fn xterm_modifier_param(modifiers: &KeyModifiers) -> u8 {
    let mut param: u8 = 1;
    if modifiers.contains(KeyModifiers::SHIFT) {
        param += 1;
    }
    if modifiers.contains(KeyModifiers::ALT) {
        param += 2;
    }
    if modifiers.contains(KeyModifiers::CONTROL) {
        param += 4;
    }
    // Note: SUPER (Cmd) はキーごとに個別処理するため、ここではエンコードしない。
    param
}

/// 矢印キーと修飾キーの組み合わせから ANSI エスケープシーケンスを組み立てる。
///
/// macOS の Cmd（Super）は一般的なターミナルと同じ挙動にマップする:
/// Cmd+Left/Right → Home/End、Cmd+Up/Down → PageUp/PageDown。
///
/// app_cursor は対象プログラムのアプリケーションカーソルキーモード（DECCKM）
/// を表す。これが有効で修飾キーが押されていない場合、矢印キーは CSI
/// （ESC [ A）ではなく SS3（ESC O A）として送る — ページャやエディタが
/// バインドしている形式で、less や bat での矢印キースクロールが効くのは
/// このためである。修飾キーがある場合、DECCKM に関わらず xterm は常に
/// CSI の 1;<param> 形式を使う。
fn arrow_with_modifiers(dir: u8, modifiers: &KeyModifiers, app_cursor: bool) -> Vec<u8> {
    // Cmd+矢印 → Home/End/PageUp/PageDown（macOS の慣習）。
    if modifiers.contains(KeyModifiers::SUPER) {
        return match dir {
            b'D' => b"\x1b[H".to_vec(),  // Cmd+左 → Home
            b'C' => b"\x1b[F".to_vec(),  // Cmd+右 → End
            b'A' => b"\x1b[5~".to_vec(), // Cmd+上 → PageUp
            b'B' => b"\x1b[6~".to_vec(), // Cmd+下 → PageDown
            _ => vec![0x1b, b'[', dir],
        };
    }

    let param = xterm_modifier_param(modifiers);
    if param == 1 {
        if app_cursor {
            vec![0x1b, b'O', dir]
        } else {
            vec![0x1b, b'[', dir]
        }
    } else {
        format!("\x1b[1;{param}{}", dir as char).into_bytes()
    }
}

/// 「tilde」形式のキー（Delete、Insert、PageUp など）と修飾キーから ANSI
/// シーケンスを組み立てる。
///
/// 修飾キーなし: ESC [ <num> ~（Delete なら \x1b[3~）。
/// 修飾キーあり: ESC [ <num> ; <param> ~。
/// 特殊ケース: Alt+Delete → ESC + d（単語単位の前方削除）。
fn tilde_key_with_modifiers(num: u8, modifiers: &KeyModifiers) -> Vec<u8> {
    // Alt+Delete → 単語単位の前方削除（readline の慣習）。
    if num == 3
        && modifiers.contains(KeyModifiers::ALT)
        && !modifiers.contains(KeyModifiers::CONTROL)
    {
        return vec![0x1b, b'd'];
    }

    let param = xterm_modifier_param(modifiers);
    if param == 1 {
        format!("\x1b[{num}~").into_bytes()
    } else {
        format!("\x1b[{num};{param}~").into_bytes()
    }
}

/// ファンクションキー（F1〜F12）と修飾キーから ANSI シーケンスを組み立てる。
fn f_key_to_ansi(n: u8, modifiers: &KeyModifiers) -> Vec<u8> {
    // ファンクションキー番号を SS3/CSI コードに対応付ける。
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
        _ => return vec![],
    };

    let param = xterm_modifier_param(modifiers);
    if code == '~' {
        // tilde 形式: ESC [ <num> ; <param> ~
        if param == 1 {
            format!("\x1b{prefix}~").into_bytes()
        } else {
            format!("\x1b{prefix};{param}~").into_bytes()
        }
    } else {
        // SS3 形式: F1-F4。修飾キーがある場合は CSI 1 ; <param> <code> を使う。
        if param == 1 {
            format!("\x1b{prefix}{code}").into_bytes()
        } else {
            format!("\x1b[1;{param}{code}").into_bytes()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn arrows_use_csi_in_normal_cursor_mode() {
        assert_eq!(
            key_event_to_ansi(&key(KeyCode::Up), false),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            key_event_to_ansi(&key(KeyCode::Down), false),
            Some(b"\x1b[B".to_vec())
        );
        assert_eq!(
            key_event_to_ansi(&key(KeyCode::Right), false),
            Some(b"\x1b[C".to_vec())
        );
        assert_eq!(
            key_event_to_ansi(&key(KeyCode::Left), false),
            Some(b"\x1b[D".to_vec())
        );
    }

    #[test]
    fn arrows_use_ss3_in_application_cursor_mode() {
        // DECCKM が有効な場合、less/bat/vim などのページャやエディタは SS3 を
        // 期待する — bat で矢印キースクロールが効くのはこのためである。
        assert_eq!(
            key_event_to_ansi(&key(KeyCode::Up), true),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(
            key_event_to_ansi(&key(KeyCode::Down), true),
            Some(b"\x1bOB".to_vec())
        );
        assert_eq!(
            key_event_to_ansi(&key(KeyCode::Right), true),
            Some(b"\x1bOC".to_vec())
        );
        assert_eq!(
            key_event_to_ansi(&key(KeyCode::Left), true),
            Some(b"\x1bOD".to_vec())
        );
    }

    #[test]
    fn home_end_honor_application_cursor_mode() {
        assert_eq!(
            key_event_to_ansi(&key(KeyCode::Home), false),
            Some(b"\x1b[H".to_vec())
        );
        assert_eq!(
            key_event_to_ansi(&key(KeyCode::End), false),
            Some(b"\x1b[F".to_vec())
        );
        assert_eq!(
            key_event_to_ansi(&key(KeyCode::Home), true),
            Some(b"\x1bOH".to_vec())
        );
        assert_eq!(
            key_event_to_ansi(&key(KeyCode::End), true),
            Some(b"\x1bOF".to_vec())
        );
    }

    #[test]
    fn modified_arrows_stay_csi_regardless_of_cursor_mode() {
        // 修飾キーがある場合、DECCKM に関わらず xterm は CSI の 1;<param> 形式を使う。
        let shift_up = KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT);
        assert_eq!(
            key_event_to_ansi(&shift_up, true),
            Some(b"\x1b[1;2A".to_vec())
        );
        assert_eq!(
            key_event_to_ansi(&shift_up, false),
            Some(b"\x1b[1;2A".to_vec())
        );
    }
}
