//! vt100 screen access, resize/reflow, alt-screen nudging, and Claude Code
//! input-waiting detection.

use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::PtySize;

use super::{PtyManager, SessionKind};

impl PtyManager {
    /// Check whether the session at `idx` has produced any visible output
    /// (i.e. the vt100 screen is not entirely blank).
    pub fn session_has_visible_output(&self, idx: usize) -> bool {
        self.sessions.get(idx).is_some_and(|s| {
            let parser = s.screen.lock().unwrap_or_else(|e| e.into_inner());
            let screen = parser.screen();
            let cols = screen.size().1;
            for row in 0..screen.size().0 {
                let row_text = Self::extract_row_text(screen, row, cols);
                if !row_text.trim().is_empty() {
                    return true;
                }
            }
            false
        })
    }

    /// Get a snapshot of the output buffer for the session at the given index.
    pub fn get_output(&self, idx: usize) -> Vec<String> {
        self.sessions
            .get(idx)
            .map(|s| {
                let buf = s.output_buffer.lock().unwrap_or_else(|e| e.into_inner());
                buf.clone()
            })
            .unwrap_or_default()
    }

    /// Whether the session at `idx` has application cursor keys mode (DECCKM)
    /// enabled. Full-screen programs on the alternate screen — pagers (`less`,
    /// `bat`), editors (`vim`) — commonly turn this on, after which they expect
    /// the arrow keys as SS3 (`ESC O A`) and ignore the default CSI (`ESC [ A`)
    /// form. Key forwarding consults this so the arrows actually drive them.
    pub fn session_application_cursor(&self, idx: usize) -> bool {
        self.sessions.get(idx).is_some_and(|s| {
            let parser = s.screen.lock().unwrap_or_else(|e| e.into_inner());
            parser.screen().application_cursor()
        })
    }

    /// Get the vt100 screen parser for the session at the given index.
    ///
    /// Returns a clone of the `Arc` so the UI can lock it for rendering.
    pub fn get_screen(&self, idx: usize) -> Option<Arc<Mutex<vt100::Parser>>> {
        self.sessions.get(idx).map(|s| Arc::clone(&s.screen))
    }

    /// Resize both the real PTY and the vt100 parser for the session at `idx`.
    ///
    /// Returns `true` when the vt100 parser was rebuilt by replaying the raw
    /// byte history (i.e. content was reflowed at a new width). Rows-only
    /// changes, and sessions that don't record a raw history, return `false`.
    ///
    /// vt100's `set_size` does not reflow: on a column change it clears each
    /// row's wrap flag and truncates/pads rows in place, so previously wrapped
    /// lines stay wrapped at the old width. To make old (autowrapped) content
    /// follow the new width, we rebuild the parser from the recorded raw byte
    /// stream, which re-wraps as it is re-parsed. Only sessions with a
    /// `raw_history` (shells — see the field docs) take this path; everything
    /// else falls back to `set_size`, which is exactly what a real terminal
    /// does for in-place-repaint apps like Claude Code (they repaint their
    /// current frame on the SIGWINCH the PTY resize delivers).
    pub fn resize_session(&mut self, idx: usize, rows: u16, cols: u16) -> bool {
        // vt100::Parser::new requires non-zero dimensions; clamp defensively so
        // the function is robust regardless of caller discipline.
        let rows = rows.max(1);
        let cols = cols.max(1);
        let scrollback = self.inactive_scrollback;
        let Some(session) = self.sessions.get(idx) else {
            return false;
        };

        // Resize the real PTY (delivers SIGWINCH so the child re-renders its
        // live region).
        let _ = session.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });

        let mut parser = session.screen.lock().unwrap_or_else(|e| e.into_inner());
        let old_cols = parser.screen().size().1;

        // Reflow only applies on a width change, and only for sessions that
        // record a raw history. A rows-only change, or a session that opts out
        // of recording (Claude, editor), is handled in place by `set_size`.
        let reflow = old_cols != cols && session.raw_history.is_some();
        if !reflow {
            parser.set_size(rows, cols);
            return false;
        }

        // Width changed — rebuild the parser at the new width by replaying the
        // raw byte history. Holding the `screen` lock keeps this consistent
        // with the reader thread, which appends to `raw_history` and processes
        // into the parser under the same lock.
        let history = session
            .raw_history
            .as_ref()
            .expect("reflow implies raw_history is Some")
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *parser = Self::rebuild_parser(&history, rows, cols, scrollback);
        true
    }

    /// Send periodic SIGWINCH nudges to sessions that recently entered
    /// alternate screen mode.  Programs like fzf may not render their
    /// initial UI until they receive a resize signal, and a single nudge
    /// can arrive before the program is ready.  This method sends nudges
    /// every ~100 ms for 500 ms after the transition, working around
    /// macOS PTY buffering quirks.
    pub fn nudge_alt_screen_sessions(&mut self) {
        const NUDGE_WINDOW: Duration = Duration::from_millis(500);
        const NUDGE_INTERVAL: Duration = Duration::from_millis(100);

        for session in &mut self.sessions {
            // Check if the reader thread detected a new alt-screen entry.
            if session.alt_screen_entered.swap(false, Ordering::Relaxed) {
                session.alt_screen_nudge_until = Some(Instant::now() + NUDGE_WINDOW);
                session.last_nudge_time = None;
            }

            // Send periodic nudges while within the window.
            let Some(until) = session.alt_screen_nudge_until else {
                continue;
            };
            if Instant::now() > until {
                session.alt_screen_nudge_until = None;
                continue;
            }

            let should_nudge = match session.last_nudge_time {
                None => true,
                Some(t) => t.elapsed() >= NUDGE_INTERVAL,
            };
            if should_nudge {
                session.last_nudge_time = Some(Instant::now());
                let (rows, cols) = {
                    let parser = session.screen.lock().unwrap_or_else(|e| e.into_inner());
                    parser.screen().size()
                };
                // macOS only delivers SIGWINCH when the size actually changes,
                // so we briefly shrink by one row then restore the real size.
                if rows > 1 {
                    let _ = session.master.resize(PtySize {
                        rows: rows - 1,
                        cols,
                        pixel_width: 0,
                        pixel_height: 0,
                    });
                }
                let _ = session.master.resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                });
            }
        }
    }

    // -- Input waiting detection ---------------------------------------------

    /// Check whether the Claude Code session at `idx` appears to be waiting
    /// for user input (idle prompt or tool-permission prompt).
    ///
    /// Returns `true` when **both** conditions are met:
    /// 1. No PTY output has been received for at least 1.5 seconds.
    /// 2. The cursor row of the vt100 screen matches a known prompt pattern.
    pub fn is_waiting_for_input(&self, idx: usize) -> bool {
        let session = match self.sessions.get(idx) {
            Some(s) => s,
            None => return false,
        };

        // Only applies to Claude Code sessions.
        if session.kind != SessionKind::ClaudeCode {
            return false;
        }

        // Condition 1: output must have been stable for ≥ 1.5s.
        const IDLE_THRESHOLD: std::time::Duration = std::time::Duration::from_millis(1500);
        {
            let t = session
                .last_output_time
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if t.elapsed() < IDLE_THRESHOLD {
                return false;
            }
        }

        // Condition 2: cursor row matches a prompt pattern.
        let parser = session.screen.lock().unwrap_or_else(|e| e.into_inner());
        let screen = parser.screen();
        let cursor_row = screen.cursor_position().0;
        let cols = screen.size().1;
        let row_text = Self::extract_row_text(screen, cursor_row, cols);
        let trimmed = row_text.trim();

        // Match: "> " prompt (Claude Code standard input)
        if trimmed.starts_with("> ") || trimmed == ">" {
            return true;
        }

        // Match: tool permission prompts containing [Y/n] or [y/N]
        if trimmed.contains("[Y/n]") || trimmed.contains("[y/N]") {
            return true;
        }

        false
    }

    /// Extract the text content of a single row from the vt100 screen.
    fn extract_row_text(screen: &vt100::Screen, row: u16, cols: u16) -> String {
        let mut text = String::with_capacity(cols as usize);
        for col in 0..cols {
            let cell = screen.cell(row, col);
            if let Some(cell) = cell {
                text.push_str(&cell.contents());
            } else {
                text.push(' ');
            }
        }
        text
    }

    /// Build a fresh vt100 parser of the given size by replaying the recorded
    /// raw byte history, re-wrapping content at the new width. This is the core
    /// of `resize_session`'s reflow path, factored out so it can be unit-tested
    /// without spawning a real PTY.
    pub(super) fn rebuild_parser(
        history: &VecDeque<u8>,
        rows: u16,
        cols: u16,
        scrollback: usize,
    ) -> vt100::Parser {
        let mut parser = vt100::Parser::new(rows, cols, scrollback);
        let (front, back) = history.as_slices();
        parser.process(front);
        parser.process(back);
        parser
    }
}
