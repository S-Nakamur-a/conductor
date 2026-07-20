//! Writing input to a PTY session: raw bytes, chunked large payloads,
//! sanitized clipboard pastes, and mouse-wheel scroll forwarding.

use std::io::Write;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};

use super::locale::utf8_chunks;
use super::PtyManager;

impl PtyManager {
    /// Send input data to the PTY at the given session index.
    pub fn write_to_session(&mut self, idx: usize, data: &[u8]) -> Result<()> {
        let session = self
            .sessions
            .get_mut(idx)
            .context("Session index out of bounds")?;
        let mut writer = session.writer.lock().unwrap_or_else(|e| e.into_inner());
        writer.write_all(data).context("Failed to write to PTY")?;
        writer.flush().context("Failed to flush PTY writer")?;
        Ok(())
    }

    /// Forward a mouse-wheel scroll to a PTY session that owns the screen,
    /// returning `true` when the scroll was handled (the caller must then **not**
    /// adjust the local scrollback offset).
    ///
    /// Three cases, matching how tmux / iTerm2 behave:
    ///
    /// 1. **Child requested mouse reporting** (vim/neovim with `mouse=`,
    ///    `less --mouse`, fzf, …): forward the wheel as a properly encoded mouse
    ///    event (SGR `1006` or legacy X10) at `col`/`row` so the application
    ///    scrolls itself. This is the fix for full-screen apps where the wheel
    ///    used to be swallowed entirely. Applies on both the normal and
    ///    alternate screen — if the app turned mouse reporting on, it wants the
    ///    event.
    /// 2. **Alternate screen, no mouse reporting** (pagers like `less`, `bat`,
    ///    `man`): the alternate screen has no scrollback of its own, so translate
    ///    each wheel notch into `lines` Up/Down arrow presses sent to the child
    ///    (the classic "alternate-scroll").
    /// 3. **Normal screen, no mouse reporting**: not handled here — return
    ///    `false` so the caller scrolls the panel's local scrollback buffer.
    ///
    /// `col` / `row` are 1-based coordinates within the PTY grid, used only for
    /// the mouse-event encoding in case 1.
    pub fn forward_scroll_to_session(
        &mut self,
        idx: usize,
        lines: usize,
        up: bool,
        col: u16,
        row: u16,
    ) -> bool {
        // Read the relevant terminal modes, then drop the session/parser
        // borrow before writing (write_to_session needs &mut self).
        let (is_alt, app_cursor, mouse_mode, mouse_encoding) = {
            let Some(session) = self.sessions.get(idx) else {
                return false;
            };
            let parser = session.screen.lock().unwrap_or_else(|e| e.into_inner());
            let screen = parser.screen();
            (
                screen.alternate_screen(),
                screen.application_cursor(),
                screen.mouse_protocol_mode(),
                screen.mouse_protocol_encoding(),
            )
        };

        // Case 1: the child captures the wheel itself — hand it an encoded event.
        if mouse_mode != vt100::MouseProtocolMode::None {
            let seq = encode_mouse_wheel(up, col, row, mouse_encoding);
            if let Err(e) = self.write_to_session(idx, &seq) {
                log::warn!("failed to forward wheel event to PTY session: {e}");
            }
            return true;
        }

        // Case 3: ordinary screen with no mouse reporting → caller scrolls
        // the local scrollback buffer.
        if !is_alt {
            return false;
        }

        // Case 2: alternate-screen pager → synthesize arrow keys.
        let arrow = scroll_arrow_sequence(up, app_cursor);
        let mut buf = Vec::with_capacity(arrow.len() * lines);
        for _ in 0..lines {
            buf.extend_from_slice(arrow);
        }
        if let Err(e) = self.write_to_session(idx, &buf) {
            log::warn!("failed to inject scroll arrows to PTY session: {e}");
        }
        true
    }

    /// Send a large text payload to the PTY as regular typed input (no
    /// bracketed paste) using chunked writes to avoid hitting the kernel's
    /// PTY input buffer limit (typically 4096 bytes on macOS / Linux).
    ///
    /// This is used for programmatic prompt injection (e.g. smart worktree)
    /// where we want the text to be displayed in full by the receiving
    /// application instead of being collapsed as a paste event.
    pub fn write_chunked_to_session(&mut self, idx: usize, text: &str) -> Result<()> {
        const CHUNK_SIZE: usize = 1024;
        const CHUNK_DELAY: Duration = Duration::from_millis(5);

        let session = self
            .sessions
            .get_mut(idx)
            .context("Session index out of bounds")?;
        let mut writer = session.writer.lock().unwrap_or_else(|e| e.into_inner());

        // Write the payload in small chunks (no bracketed paste markers).
        // Chunk on UTF-8 character boundaries: a chunk that ends mid-character
        // would be flushed (and, at the chunk limit, followed by a delay) with a
        // truncated multi-byte sequence, which the receiving application can
        // mis-decode — corrupting full-width / multi-byte input.
        for chunk in utf8_chunks(text, CHUNK_SIZE) {
            writer
                .write_all(chunk.as_bytes())
                .context("Failed to write chunk to PTY")?;
            writer.flush().context("Failed to flush PTY writer")?;
            if chunk.len() == CHUNK_SIZE {
                thread::sleep(CHUNK_DELAY);
            }
        }

        Ok(())
    }

    /// Send a clipboard paste payload to the PTY, sanitizing it first and using
    /// chunked writes to avoid hitting the kernel's PTY input buffer limit
    /// (typically 4096 bytes on macOS / Linux).
    ///
    /// Two safety steps mirror what a well-behaved terminal does with a paste:
    ///
    /// 1. **Sanitize** (`sanitize_pasted_text`): clipboard content can carry
    ///    ANSI escape sequences and other non-printable control bytes (copied
    ///    from a colorized terminal, a web page, etc.). Forwarding those raw
    ///    lets them move the cursor, change modes, or — worst — smuggle a
    ///    `\x1b[201~` that ends bracketed paste early so the remainder runs as
    ///    typed commands. We strip escape sequences and control characters,
    ///    keeping only tabs and newlines (CR is normalized to LF).
    /// 2. **Conditional bracketing**: the `\x1b[200~` / `\x1b[201~` markers are
    ///    only emitted when the foreground application has actually enabled
    ///    bracketed paste (DECSET 2004), exactly as a real terminal gates them.
    ///    Wrapping unconditionally would dump literal `[200~` / `[201~` text
    ///    into apps that never asked for it (a bare prompt, `cat`, …).
    pub fn write_paste_to_session(&mut self, idx: usize, text: &str) -> Result<()> {
        const CHUNK_SIZE: usize = 1024;
        const CHUNK_DELAY: Duration = Duration::from_millis(5);

        let cleaned = sanitize_pasted_text(text);

        let session = self
            .sessions
            .get_mut(idx)
            .context("Session index out of bounds")?;

        // Read the bracketed-paste mode flag under the screen lock, then drop it
        // before taking the writer lock.
        let bracketed = {
            let parser = session.screen.lock().unwrap_or_else(|e| e.into_inner());
            parser.screen().bracketed_paste()
        };

        let mut writer = session.writer.lock().unwrap_or_else(|e| e.into_inner());

        // Begin bracketed paste mode (only if the app understands it).
        if bracketed {
            writer
                .write_all(b"\x1b[200~")
                .context("Failed to write paste-start to PTY")?;
            writer.flush().context("Failed to flush PTY writer")?;
        }

        // Write the payload in small chunks. Split on UTF-8 character
        // boundaries so a flushed chunk never ends with a truncated multi-byte
        // sequence (see `utf8_chunks`) — otherwise full-width / multi-byte text
        // split across the 1 KiB boundary can be mis-decoded by the receiver.
        for chunk in utf8_chunks(&cleaned, CHUNK_SIZE) {
            writer
                .write_all(chunk.as_bytes())
                .context("Failed to write chunk to PTY")?;
            writer.flush().context("Failed to flush PTY writer")?;
            if chunk.len() == CHUNK_SIZE {
                thread::sleep(CHUNK_DELAY);
            }
        }

        // End bracketed paste mode.
        if bracketed {
            writer
                .write_all(b"\x1b[201~")
                .context("Failed to write paste-end to PTY")?;
            writer.flush().context("Failed to flush PTY writer")?;
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Free helper functions
// ---------------------------------------------------------------------------

/// Sanitize clipboard text before it is written to a PTY as a paste.
///
/// Clipboard content frequently carries bytes that are unsafe to forward
/// verbatim into a terminal's input stream:
/// * **ANSI escape sequences** (copied from a colorized terminal, a TUI, a web
///   page that styled its text): these move the cursor, switch modes, or — most
///   dangerously — can contain a `\x1b[201~` that prematurely *ends* bracketed
///   paste, after which the rest of the clipboard is interpreted as typed
///   commands. Whole escape sequences are dropped.
/// * **Other C0/C1 control characters and DEL**: forwarded raw they can ring
///   bells, send signals (via the line discipline), or corrupt the input.
///
/// What is preserved: ordinary printable text, **tabs** (`\t`), and **newlines**
/// (`\n`). Carriage returns are normalized — `\r\n` and lone `\r` both become a
/// single `\n` — so multi-line pastes keep their line structure without
/// injecting bare CRs.
pub(super) fn sanitize_pasted_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // ESC introduces an escape sequence — drop the whole thing.
            '\u{1b}' => skip_escape_sequence(&mut chars),
            '\t' | '\n' => out.push(c),
            // Normalize CR / CRLF to a single LF.
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            // Drop every other control character (remaining C0, DEL, C1).
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Consume the remainder of an ANSI escape sequence whose introducing `ESC` has
/// already been read from `chars`. Handles the common sequence shapes so that no
/// stray bytes of a dropped sequence leak into the sanitized output:
/// * `CSI` (`ESC [`) — parameters/intermediates up to a final byte `0x40..=0x7E`.
/// * String sequences `OSC/DCS/SOS/PM/APC` (`ESC ] P X ^ _`) — up to `BEL` or
///   the String Terminator `ESC \`.
/// * `SS2/SS3` (`ESC N` / `ESC O`) — exactly one following byte.
/// * Anything else (`ESC` + a single byte, e.g. `ESC 7`) — already consumed.
fn skip_escape_sequence(chars: &mut std::iter::Peekable<std::str::Chars>) {
    match chars.next() {
        Some('[') => {
            for c in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    break;
                }
            }
        }
        Some(']') | Some('P') | Some('X') | Some('^') | Some('_') => {
            while let Some(c) = chars.next() {
                if c == '\u{07}' {
                    break;
                }
                if c == '\u{1b}' {
                    if chars.peek() == Some(&'\\') {
                        chars.next();
                    }
                    break;
                }
            }
        }
        Some('N') | Some('O') => {
            // Single-shift: skip the one character it selects.
            chars.next();
        }
        _ => {}
    }
}

/// Encode a single mouse-wheel notch as the byte sequence a terminal sends to a
/// child program that has enabled mouse reporting.
///
/// `up` selects wheel-up (xterm button 64) vs wheel-down (65). `col` / `row` are
/// 1-based cell coordinates. The `encoding` follows the child's requested mode:
/// SGR (`1006`, the modern default that has no 223-column limit) emits
/// `CSI < b ; col ; row M`; otherwise the legacy X10 form `CSI M Cb Cx Cy` is
/// used, with each value offset by 32 and clamped to one byte.
pub(super) fn encode_mouse_wheel(
    up: bool,
    col: u16,
    row: u16,
    encoding: vt100::MouseProtocolEncoding,
) -> Vec<u8> {
    let button: u16 = if up { 64 } else { 65 };
    let col = col.max(1);
    let row = row.max(1);
    match encoding {
        vt100::MouseProtocolEncoding::Sgr => {
            format!("\x1b[<{button};{col};{row}M").into_bytes()
        }
        // Default (X10) and Utf8: CSI M Cb Cx Cy, each byte offset by 32.
        // Values above 223 cannot be represented in the legacy form; clamp so
        // we never emit a byte that wraps the coordinate.
        _ => {
            let cb = (32 + button).min(255) as u8;
            let cx = (32 + col).min(255) as u8;
            let cy = (32 + row).min(255) as u8;
            vec![0x1b, b'[', b'M', cb, cx, cy]
        }
    }
}

/// Return the escape sequence for an Up/Down arrow key press used to scroll a
/// pager on the alternate screen.
///
/// `up` selects Up (`true`) vs Down (`false`). `app_cursor` honors DECCKM
/// (application cursor keys mode): when set, terminals send SS3 (`ESC O`)
/// sequences; otherwise CSI (`ESC [`). Pagers like `less` enable application
/// cursor mode and bind the SS3 forms, so respecting it is necessary for the
/// arrow keys to register reliably across programs.
pub(super) fn scroll_arrow_sequence(up: bool, app_cursor: bool) -> &'static [u8] {
    match (up, app_cursor) {
        (true, true) => b"\x1bOA",   // Up   (SS3)
        (true, false) => b"\x1b[A",  // Up   (CSI)
        (false, true) => b"\x1bOB",  // Down (SS3)
        (false, false) => b"\x1b[B", // Down (CSI)
    }
}
