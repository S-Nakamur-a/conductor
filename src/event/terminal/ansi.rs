//! `KeyEvent` → ANSI byte sequence conversion for PTY forwarding.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Convert a crossterm `KeyEvent` into the ANSI byte sequence that a real
/// terminal would send to a child process.
///
/// Returns `None` for key events that have no meaningful byte representation
/// (e.g. bare modifier key presses like Shift alone).
pub(super) fn key_event_to_ansi(key: &KeyEvent, app_cursor: bool) -> Option<Vec<u8>> {
    let mods = key.modifiers;
    let data = match key.code {
        // ── Character keys ───────────────────────────────────────
        KeyCode::Char(c) => char_with_modifiers(c, mods),

        // ── Enter / Tab ──────────────────────────────────────────
        KeyCode::Enter => {
            if mods.contains(KeyModifiers::SHIFT) {
                // Shift+Enter → CSI u so Claude Code treats it as newline.
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

        // ── Backspace / Delete ───────────────────────────────────
        KeyCode::Backspace => {
            if mods.contains(KeyModifiers::SUPER) {
                // Cmd+Backspace → delete to beginning of line (Ctrl+U).
                vec![0x15]
            } else if mods.contains(KeyModifiers::ALT) {
                // Option+Backspace → delete word backward (ESC DEL).
                vec![0x1b, 0x7f]
            } else if mods.contains(KeyModifiers::CONTROL) {
                // Ctrl+Backspace → delete word backward (same as Ctrl+W).
                vec![0x17]
            } else {
                vec![0x7f]
            }
        }
        KeyCode::Delete => tilde_key_with_modifiers(3, &mods),

        // ── Escape ───────────────────────────────────────────────
        KeyCode::Esc => vec![0x1b],

        // ── Arrow keys ───────────────────────────────────────────
        KeyCode::Up => arrow_with_modifiers(b'A', &mods, app_cursor),
        KeyCode::Down => arrow_with_modifiers(b'B', &mods, app_cursor),
        KeyCode::Right => arrow_with_modifiers(b'C', &mods, app_cursor),
        KeyCode::Left => arrow_with_modifiers(b'D', &mods, app_cursor),

        // ── Home / End ───────────────────────────────────────────
        // DECCKM applies to the unmodified form: SS3 (`ESC O H`) when the
        // application has application-cursor-keys mode on, CSI otherwise.
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

        // ── Page Up / Down ───────────────────────────────────────
        KeyCode::PageUp => tilde_key_with_modifiers(5, &mods),
        KeyCode::PageDown => tilde_key_with_modifiers(6, &mods),

        // ── Insert ───────────────────────────────────────────────
        KeyCode::Insert => tilde_key_with_modifiers(2, &mods),

        // ── Function keys ────────────────────────────────────────
        KeyCode::F(n) => f_key_to_ansi(n, &mods),

        // ── Modifier-only or unknown keys — no bytes to send ─────
        _ => return None,
    };

    Some(data)
}

/// Convert a character key with modifiers to bytes.
fn char_with_modifiers(c: char, mods: KeyModifiers) -> Vec<u8> {
    if mods.contains(KeyModifiers::CONTROL) {
        if c.is_ascii_lowercase() || c.is_ascii_uppercase() {
            // Ctrl+letter → control byte (Ctrl+A = 0x01, ..., Ctrl+Z = 0x1a).
            let ctrl_byte = (c.to_ascii_lowercase() as u8)
                .wrapping_sub(b'a')
                .wrapping_add(1);
            if mods.contains(KeyModifiers::ALT) {
                vec![0x1b, ctrl_byte]
            } else {
                vec![ctrl_byte]
            }
        } else {
            // Ctrl + non-letter (e.g. Ctrl+[ = ESC, Ctrl+] = 0x1d).
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
        // Alt+char → ESC prefix + char (Meta-key encoding).
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
        // Plain char or Shift+char (enhanced keyboard protocol may send
        // lowercase + SHIFT modifier — apply shift manually).
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

/// Compute the xterm modifier parameter from crossterm modifiers.
///
/// xterm encodes modifiers as `1 + bitmask` where:
///   Shift = 1, Alt = 2, Ctrl = 4, Super/Meta = 8.
/// Returns 1 when no modifiers are set.
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
    // Note: SUPER (Cmd) has special handling per-key, not encoded here.
    param
}

/// Build the ANSI escape sequence for an arrow key with modifier keys.
///
/// For Cmd (Super) on macOS, we map to the same behaviour as common
/// terminals: Cmd+Left/Right → Home/End, Cmd+Up/Down → PageUp/PageDown.
///
/// `app_cursor` reflects the target program's application-cursor-keys mode
/// (DECCKM). When it is on and no modifiers are pressed, arrow keys are sent as
/// SS3 (`ESC O A`) instead of CSI (`ESC [ A`) — what pagers/editors bind to
/// (this is what makes arrow-key scrolling work in `less`/`bat`). With
/// modifiers, xterm always uses the CSI `1;<param>` form regardless of DECCKM.
fn arrow_with_modifiers(dir: u8, modifiers: &KeyModifiers, app_cursor: bool) -> Vec<u8> {
    // Cmd+Arrow → Home/End/PageUp/PageDown (macOS convention).
    if modifiers.contains(KeyModifiers::SUPER) {
        return match dir {
            b'D' => b"\x1b[H".to_vec(),  // Cmd+Left  → Home
            b'C' => b"\x1b[F".to_vec(),  // Cmd+Right → End
            b'A' => b"\x1b[5~".to_vec(), // Cmd+Up    → PageUp
            b'B' => b"\x1b[6~".to_vec(), // Cmd+Down  → PageDown
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

/// Build the ANSI sequence for a "tilde" key (Delete, Insert, PageUp, etc.)
/// with optional modifiers.
///
/// Without modifiers: `ESC [ <num> ~` (e.g. `\x1b[3~` for Delete).
/// With modifiers: `ESC [ <num> ; <param> ~`.
/// Special case: Alt+Delete → ESC + d (word-forward delete).
fn tilde_key_with_modifiers(num: u8, modifiers: &KeyModifiers) -> Vec<u8> {
    // Alt+Delete → word-forward delete (readline convention).
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

/// Build the ANSI sequence for a function key (F1–F12) with modifiers.
fn f_key_to_ansi(n: u8, modifiers: &KeyModifiers) -> Vec<u8> {
    // Map function key number to the SS3/CSI code.
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
        // Tilde-style: ESC [ <num> ; <param> ~
        if param == 1 {
            format!("\x1b{prefix}~").into_bytes()
        } else {
            format!("\x1b{prefix};{param}~").into_bytes()
        }
    } else {
        // SS3-style: F1-F4. With modifiers, use CSI 1 ; <param> <code>.
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
        assert_eq!(key_event_to_ansi(&key(KeyCode::Up), false), Some(b"\x1b[A".to_vec()));
        assert_eq!(key_event_to_ansi(&key(KeyCode::Down), false), Some(b"\x1b[B".to_vec()));
        assert_eq!(key_event_to_ansi(&key(KeyCode::Right), false), Some(b"\x1b[C".to_vec()));
        assert_eq!(key_event_to_ansi(&key(KeyCode::Left), false), Some(b"\x1b[D".to_vec()));
    }

    #[test]
    fn arrows_use_ss3_in_application_cursor_mode() {
        // DECCKM on: pagers/editors (less, bat, vim) expect SS3 — this is what
        // makes arrow-key scrolling register in `bat`.
        assert_eq!(key_event_to_ansi(&key(KeyCode::Up), true), Some(b"\x1bOA".to_vec()));
        assert_eq!(key_event_to_ansi(&key(KeyCode::Down), true), Some(b"\x1bOB".to_vec()));
        assert_eq!(key_event_to_ansi(&key(KeyCode::Right), true), Some(b"\x1bOC".to_vec()));
        assert_eq!(key_event_to_ansi(&key(KeyCode::Left), true), Some(b"\x1bOD".to_vec()));
    }

    #[test]
    fn home_end_honor_application_cursor_mode() {
        assert_eq!(key_event_to_ansi(&key(KeyCode::Home), false), Some(b"\x1b[H".to_vec()));
        assert_eq!(key_event_to_ansi(&key(KeyCode::End), false), Some(b"\x1b[F".to_vec()));
        assert_eq!(key_event_to_ansi(&key(KeyCode::Home), true), Some(b"\x1bOH".to_vec()));
        assert_eq!(key_event_to_ansi(&key(KeyCode::End), true), Some(b"\x1bOF".to_vec()));
    }

    #[test]
    fn modified_arrows_stay_csi_regardless_of_cursor_mode() {
        // With a modifier, xterm uses the CSI `1;<param>` form even under DECCKM.
        let shift_up = KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT);
        assert_eq!(key_event_to_ansi(&shift_up, true), Some(b"\x1b[1;2A".to_vec()));
        assert_eq!(key_event_to_ansi(&shift_up, false), Some(b"\x1b[1;2A".to_vec()));
    }
}
