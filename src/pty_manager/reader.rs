//! Background PTY reader thread: feeds raw bytes to the vt100 parser,
//! maintains the line buffer used for Claude Code output analysis, answers
//! Cursor Position Report queries, and records the raw-byte history used for
//! reflow-on-resize.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::{PtyManager, MAX_RAW_HISTORY_BYTES};

impl PtyManager {
    /// Background reader thread function.
    ///
    /// Continuously reads from the PTY reader, feeds raw bytes to the vt100
    /// parser for proper terminal rendering, and also splits into lines for
    /// the line buffer used by Claude Code output analysis.
    ///
    /// The writer handle is used to respond to terminal queries such as
    /// cursor position reports (`CSI 6 n`), which many programs (fzf, shells)
    /// send to determine where to draw their UI.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn reader_thread(
        mut reader: Box<dyn Read + Send>,
        buffer: Arc<Mutex<Vec<String>>>,
        buffer_limit: Arc<Mutex<usize>>,
        screen: Arc<Mutex<vt100::Parser>>,
        raw_history: Option<Arc<Mutex<VecDeque<u8>>>>,
        last_output_time: Arc<Mutex<Instant>>,
        alt_screen_entered: Arc<AtomicBool>,
        writer: Arc<Mutex<Box<dyn Write + Send>>>,
        output_notify: Arc<AtomicBool>,
    ) {
        let mut read_buf = [0u8; 4096];
        // Partial line accumulator (for data that doesn't end with '\n').
        let mut partial = String::new();
        // Track previous alternate-screen state to detect transitions.
        let mut prev_alt_screen = false;

        loop {
            match reader.read(&mut read_buf) {
                Ok(0) => {
                    // EOF — the PTY master has been closed.
                    // Flush any remaining partial line.
                    if !partial.is_empty() {
                        let line = std::mem::take(&mut partial);
                        Self::push_line(&buffer, &buffer_limit, line);
                    }
                    break;
                }
                Ok(n) => {
                    let bytes = &read_buf[..n];

                    // Update the last output timestamp and notify the main loop.
                    {
                        let mut t = last_output_time.lock().unwrap_or_else(|e| e.into_inner());
                        *t = Instant::now();
                    }
                    output_notify.store(true, Ordering::Relaxed);

                    // Count terminal queries that need responses BEFORE
                    // feeding to the parser (the parser consumes the bytes).
                    let cpr_count = count_csi_dsr(bytes);

                    // Feed raw bytes to vt100 for proper rendering.
                    {
                        let mut parser = screen.lock().unwrap_or_else(|e| e.into_inner());
                        parser.process(bytes);

                        // Record the same bytes for reflow-on-resize, but only
                        // for sessions that opted into a raw history (shells).
                        // Done under the `screen` lock so the recorded stream
                        // stays exactly in sync with what the parser has
                        // processed, and so a concurrent `resize_session`
                        // rebuild sees a consistent history. The inner scope
                        // releases the history guard before the CPR / alt-screen
                        // work below.
                        if let Some(raw_history) = &raw_history {
                            let mut history =
                                raw_history.lock().unwrap_or_else(|e| e.into_inner());
                            history.extend(bytes.iter().copied());
                            Self::trim_raw_history(&mut history, MAX_RAW_HISTORY_BYTES);
                        }

                        // Respond to Cursor Position Report requests (CSI 6 n).
                        // Programs like fzf, zsh, and bash send this to
                        // determine the current cursor position for inline
                        // rendering.  Without a response, they block until a
                        // timeout or until the user types something.
                        if cpr_count > 0 {
                            let cursor = parser.screen().cursor_position();
                            // Terminal coordinates are 1-based.
                            let response = format!("\x1b[{};{}R", cursor.0 + 1, cursor.1 + 1,);
                            log::debug!(
                                "CPR: responding to {} query(ies) with cursor ({}, {})",
                                cpr_count,
                                cursor.0 + 1,
                                cursor.1 + 1,
                            );
                            if let Ok(mut w) = writer.lock() {
                                for _ in 0..cpr_count {
                                    let _ = w.write_all(response.as_bytes());
                                }
                                let _ = w.flush();
                            }
                        }

                        // Detect transition into alternate screen mode.
                        let is_alt = parser.screen().alternate_screen();
                        if is_alt && !prev_alt_screen {
                            log::debug!(
                                "ALT_SCREEN reader: entered alternate screen, chunk_size={n}"
                            );
                            alt_screen_entered.store(true, Ordering::Relaxed);
                        }
                        prev_alt_screen = is_alt;
                    }

                    // Also maintain line buffer for CC analysis.
                    let chunk = String::from_utf8_lossy(bytes);
                    partial.push_str(&chunk);

                    // Split on newlines and push complete lines.
                    while let Some(pos) = partial.find('\n') {
                        let line: String = partial.drain(..=pos).collect();
                        // Trim the trailing '\n' (and optional '\r').
                        let line = line
                            .trim_end_matches('\n')
                            .trim_end_matches('\r')
                            .to_string();
                        Self::push_line(&buffer, &buffer_limit, line);
                    }
                }
                Err(_) => {
                    // Read error — the PTY is likely closed; exit the thread.
                    break;
                }
            }
        }
    }

    /// Trim the raw byte history down to at most `cap` bytes, dropping from the
    /// front. After reaching the cap, try to drop up to the next newline so the
    /// retained history starts at a clean line boundary — this avoids replaying
    /// a half-line that would render incorrectly after a reflow rebuild.
    ///
    /// The newline search is bounded: an escape-sequence-heavy TUI stream can
    /// have very few newlines, and an unbounded search would either cost an
    /// O(n) scan on every append or (if it kept popping) drain the whole buffer
    /// to empty. If no newline is found nearby, the bytes are kept as-is — a
    /// slightly imperfect first line is far better than a blank screen.
    pub(super) fn trim_raw_history(history: &mut VecDeque<u8>, cap: usize) {
        if history.len() <= cap {
            return;
        }
        let excess = history.len() - cap;
        for _ in 0..excess {
            history.pop_front();
        }
        // Align to just after the next newline, if one is within the window.
        const ALIGN_SCAN_LIMIT: usize = 8 * 1024;
        if let Some(pos) = history
            .iter()
            .take(ALIGN_SCAN_LIMIT)
            .position(|&b| b == b'\n')
        {
            for _ in 0..=pos {
                history.pop_front();
            }
        }
    }

    /// Push a single line into the shared buffer, enforcing the current limit.
    fn push_line(buffer: &Arc<Mutex<Vec<String>>>, buffer_limit: &Arc<Mutex<usize>>, line: String) {
        let limit = {
            let l = buffer_limit.lock().unwrap_or_else(|e| e.into_inner());
            *l
        };

        let mut buf = buffer.lock().unwrap_or_else(|e| e.into_inner());
        buf.push(line);

        // Trim from the front if we exceed the limit.
        if buf.len() > limit {
            let excess = buf.len() - limit;
            buf.drain(..excess);
        }
    }
}

/// Count the number of Cursor Position Report requests (`CSI 6 n` = `\x1b[6n`)
/// in a byte slice.  Programs send this to ask the terminal "where is the
/// cursor?" and expect a `CSI row ; col R` response.
fn count_csi_dsr(bytes: &[u8]) -> usize {
    if bytes.len() < 4 {
        return 0;
    }
    bytes.windows(4).filter(|w| *w == b"\x1b[6n").count()
}
