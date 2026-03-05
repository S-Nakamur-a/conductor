//! PTY session management.
//!
//! Uses `portable-pty` to spawn and manage pseudo-terminal sessions so that
//! users can run shell commands or Claude Code directly inside the TUI.
//!
//! Each session is backed by a real pseudo-terminal, with a background reader
//! thread that captures output into a bounded line buffer.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// SessionKind
// ---------------------------------------------------------------------------

/// The kind of process running inside a PTY session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    /// A `claude` CLI session (Claude Code).
    ClaudeCode,
    /// An interactive shell session (e.g. bash, zsh, fish).
    Shell,
}

// ---------------------------------------------------------------------------
// PtySession
// ---------------------------------------------------------------------------

/// A single PTY session with its associated reader/writer handles.
pub struct PtySession {
    /// Unique identifier (UUID v4).
    pub id: String,
    /// Human-readable label for this session (e.g. "Auth logic implementation").
    pub label: String,
    /// What kind of process is running.
    pub kind: SessionKind,
    /// The worktree name this session is associated with.
    pub worktree: String,
    /// The working directory this session was spawned in.
    pub working_dir: PathBuf,
    /// Whether this session is the currently displayed (active) session.
    pub is_active: bool,

    // -- PTY handles -------------------------------------------------------

    /// The master end of the PTY; used for resize operations.
    master: Box<dyn portable_pty::MasterPty + Send>,
    /// Writer handle for sending input to the PTY.
    /// Shared with the reader thread so it can respond to terminal queries
    /// (e.g. cursor position reports) with minimal latency.
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    /// The child process spawned inside the PTY.
    child: Box<dyn portable_pty::Child + Send + Sync>,

    // -- Output buffer (shared with the reader thread) ---------------------

    /// Lines of captured output, shared with the background reader thread.
    output_buffer: Arc<Mutex<Vec<String>>>,
    /// Current maximum number of lines to retain.
    max_buffer_lines: usize,

    // -- vt100 terminal emulator -------------------------------------------

    /// A vt100 parser that processes raw PTY bytes for proper terminal rendering.
    screen: Arc<Mutex<vt100::Parser>>,

    // -- Input waiting detection ------------------------------------------

    /// Timestamp of the last PTY output received. Shared with the reader thread.
    pub last_output_time: Arc<Mutex<Instant>>,

    // -- Alternate screen detection ----------------------------------------

    /// Set to `true` by the reader thread when a transition *into* alternate
    /// screen mode is detected.  The main loop can check this flag and send
    /// a no-op resize (SIGWINCH) to nudge the child into re-rendering.
    pub alt_screen_entered: Arc<AtomicBool>,

    /// Deadline until which periodic SIGWINCH nudges should be sent.
    /// Set when `alt_screen_entered` is first observed by the main loop.
    alt_screen_nudge_until: Option<Instant>,
    /// Timestamp of the last SIGWINCH nudge sent, used for throttling.
    last_nudge_time: Option<Instant>,
}

// ---------------------------------------------------------------------------
// PtyManager
// ---------------------------------------------------------------------------

/// Manages one or more PTY sessions.
pub struct PtyManager {
    pty_system: NativePtySystem,
    sessions: Vec<PtySession>,
    /// Parallel vector of buffer-limit handles shared with reader threads.
    /// Each entry corresponds to the session at the same index in `sessions`.
    buffer_limits: Vec<Arc<Mutex<usize>>>,
    /// Scrollback lines for the active (foreground) session.
    active_scrollback: usize,
    /// Scrollback lines for inactive (background) sessions.
    inactive_scrollback: usize,
}

impl PtyManager {
    /// Create a new `PtyManager` with no sessions, using the given scrollback limits.
    pub fn new(active_scrollback: usize, inactive_scrollback: usize) -> Self {
        Self {
            pty_system: NativePtySystem::default(),
            sessions: Vec::new(),
            buffer_limits: Vec::new(),
            active_scrollback,
            inactive_scrollback,
        }
    }

    /// Spawn a new PTY session and return its index in the session list.
    ///
    /// * `kind` — whether to launch Claude Code or a shell.
    /// * `worktree` — the worktree name this session belongs to.
    /// * `label` — a human-readable label shown in the UI.
    /// * `shell_path` — path to the shell binary (used only for `SessionKind::Shell`).
    /// * `working_dir` — the working directory for the spawned process.
    /// * `rows` — number of rows for the PTY and vt100 parser.
    /// * `cols` — number of columns for the PTY and vt100 parser.
    /// * `resume_session_id` — if `Some`, pass `--resume <id>` to the Claude CLI.
    /// * `repo_root` — the repository root path, used to set `CONDUCTOR_DB_PATH`
    ///   for Claude Code sessions so the MCP server can locate the database.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_session(
        &mut self,
        kind: SessionKind,
        worktree: &str,
        label: &str,
        shell_path: &str,
        working_dir: &PathBuf,
        rows: u16,
        cols: u16,
        resume_session_id: Option<&str>,
        repo_root: &Path,
    ) -> Result<usize> {
        // 1. Open a new PTY pair with the given size.
        let pair = self
            .pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("Failed to open PTY pair")?;

        // 2. Build the command depending on the session kind.
        let mut cmd = match kind {
            SessionKind::ClaudeCode => {
                let mut c = CommandBuilder::new("claude");
                if let Some(resume_id) = resume_session_id {
                    c.arg("--resume");
                    c.arg(resume_id);
                }
                // Let the conductor MCP server find the review database.
                let db_path = repo_root.join(".conductor").join("conductor.db");
                c.env("CONDUCTOR_DB_PATH", db_path);
                c
            }
            SessionKind::Shell => CommandBuilder::new(shell_path),
        };
        cmd.cwd(working_dir);

        // 3. Spawn the child process on the slave end.
        let child = pair
            .slave
            .spawn_command(cmd)
            .context("Failed to spawn command in PTY")?;

        // 4. Obtain reader and writer handles from the master end.
        let reader = pair
            .master
            .try_clone_reader()
            .context("Failed to clone PTY reader")?;
        let writer: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(
            pair.master
                .take_writer()
                .context("Failed to take PTY writer")?,
        ));
        let writer_for_thread = Arc::clone(&writer);

        // 5. Set up the shared output buffer.
        let output_buffer: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let max_buffer_lines = self.inactive_scrollback;

        // 5b. Create the vt100 parser with the same size as the PTY.
        let screen: Arc<Mutex<vt100::Parser>> =
            Arc::new(Mutex::new(vt100::Parser::new(rows, cols, self.inactive_scrollback)));

        // 6. Spawn a background thread that continuously reads PTY output.
        let buffer_clone = Arc::clone(&output_buffer);
        let screen_clone = Arc::clone(&screen);
        // We store max_buffer_lines in the session, but the reader thread
        // needs its own reference. We use a separate Arc<Mutex<usize>> so
        // that set_active() can dynamically adjust the limit.
        let buffer_limit = Arc::new(Mutex::new(max_buffer_lines));
        let buffer_limit_for_thread = Arc::clone(&buffer_limit);

        // Track when the last output was received (for input-waiting detection).
        let last_output_time = Arc::new(Mutex::new(Instant::now()));
        let last_output_time_for_thread = Arc::clone(&last_output_time);

        // Track alternate-screen transitions so the main loop can nudge
        // programs (e.g. fzf) that may not have rendered their initial UI.
        let alt_screen_entered = Arc::new(AtomicBool::new(false));
        let alt_screen_entered_for_thread = Arc::clone(&alt_screen_entered);

        thread::Builder::new()
            .name(format!("pty-reader-{label}"))
            .spawn(move || {
                Self::reader_thread(
                    reader,
                    buffer_clone,
                    buffer_limit_for_thread,
                    screen_clone,
                    last_output_time_for_thread,
                    alt_screen_entered_for_thread,
                    writer_for_thread,
                );
            })
            .context("Failed to spawn PTY reader thread")?;

        // 7. Build the session struct.
        let session = PtySession {
            id: Uuid::new_v4().to_string(),
            label: label.to_string(),
            kind,
            worktree: worktree.to_string(),
            working_dir: working_dir.clone(),
            is_active: false,
            master: pair.master,
            writer,
            child,
            output_buffer,
            max_buffer_lines,
            screen,
            last_output_time,
            alt_screen_entered,
            alt_screen_nudge_until: None,
            last_nudge_time: None,
        };

        self.sessions.push(session);
        let idx = self.sessions.len() - 1;

        // Store the buffer limit Arc so that set_active() can dynamically
        // adjust it for the reader thread.
        self.buffer_limits.push(buffer_limit);

        Ok(idx)
    }

    /// Send input data to the PTY at the given session index.
    pub fn write_to_session(&mut self, idx: usize, data: &[u8]) -> Result<()> {
        let session = self
            .sessions
            .get_mut(idx)
            .context("Session index out of bounds")?;
        let mut writer = session
            .writer
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        writer
            .write_all(data)
            .context("Failed to write to PTY")?;
        writer
            .flush()
            .context("Failed to flush PTY writer")?;
        Ok(())
    }

    /// Send a large text payload to the PTY using bracketed paste mode and
    /// chunked writes to avoid hitting the kernel's PTY input buffer limit
    /// (typically 4096 bytes on macOS / Linux).
    pub fn write_paste_to_session(&mut self, idx: usize, text: &str) -> Result<()> {
        const CHUNK_SIZE: usize = 1024;
        const CHUNK_DELAY: Duration = Duration::from_millis(5);

        let session = self
            .sessions
            .get_mut(idx)
            .context("Session index out of bounds")?;
        let mut writer = session
            .writer
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        // Begin bracketed paste mode.
        writer
            .write_all(b"\x1b[200~")
            .context("Failed to write paste-start to PTY")?;
        writer.flush().context("Failed to flush PTY writer")?;

        // Write the payload in small chunks.
        let data = text.as_bytes();
        for chunk in data.chunks(CHUNK_SIZE) {
            writer
                .write_all(chunk)
                .context("Failed to write chunk to PTY")?;
            writer.flush().context("Failed to flush PTY writer")?;
            if chunk.len() == CHUNK_SIZE {
                thread::sleep(CHUNK_DELAY);
            }
        }

        // End bracketed paste mode.
        writer
            .write_all(b"\x1b[201~")
            .context("Failed to write paste-end to PTY")?;
        writer.flush().context("Failed to flush PTY writer")?;

        Ok(())
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

    /// Get the vt100 screen parser for the session at the given index.
    ///
    /// Returns a clone of the `Arc` so the UI can lock it for rendering.
    pub fn get_screen(&self, idx: usize) -> Option<Arc<Mutex<vt100::Parser>>> {
        self.sessions.get(idx).map(|s| Arc::clone(&s.screen))
    }

    /// Resize both the real PTY and the vt100 parser for the session at `idx`.
    pub fn resize_session(&mut self, idx: usize, rows: u16, cols: u16) {
        if let Some(session) = self.sessions.get(idx) {
            // Resize the real PTY.
            let _ = session.master.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            });
            // Resize the vt100 parser.
            let mut parser = session.screen.lock().unwrap_or_else(|e| e.into_inner());
            parser.set_size(rows, cols);
        }
    }

    /// Activate a session without deactivating any other session.
    /// Used in the unified layout where Claude and Shell sessions can be
    /// simultaneously active.
    pub fn activate_session(&mut self, idx: usize) {
        if let Some(session) = self.sessions.get_mut(idx) {
            session.is_active = true;
            session.max_buffer_lines = self.active_scrollback;
        }
        if let Some(limit) = self.buffer_limits.get(idx) {
            let mut l = limit.lock().unwrap_or_else(|e| e.into_inner());
            *l = self.active_scrollback;
        }
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

    /// Return the number of sessions.
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Read-only access to the sessions slice.
    pub fn sessions(&self) -> &[PtySession] {
        &self.sessions
    }

    /// Kill the child process for the session at the given index.
    pub fn kill_session(&mut self, idx: usize) -> Result<()> {
        let session = self
            .sessions
            .get_mut(idx)
            .context("Session index out of bounds")?;
        session
            .child
            .kill()
            .map_err(|e| anyhow::anyhow!("Failed to kill session child process: {e}"))?;
        Ok(())
    }

    /// Remove the session at `idx`, cleaning up resources.
    ///
    /// Dropping the session closes the PTY master, which causes the
    /// background reader thread to see EOF and exit.
    pub fn remove_session(&mut self, idx: usize) {
        if idx < self.sessions.len() {
            self.sessions.remove(idx);
            self.buffer_limits.remove(idx);
        }
    }

    /// Check whether the child process for the session at `idx` is still
    /// running.
    pub fn is_session_alive(&mut self, idx: usize) -> bool {
        self.sessions
            .get_mut(idx)
            .map(|s| {
                match s.child.try_wait() {
                    Ok(Some(_exit_status)) => false, // exited
                    Ok(None) => true,                // still running
                    Err(_) => false,                 // treat errors as dead
                }
            })
            .unwrap_or(false)
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

    /// Background reader thread function.
    ///
    /// Continuously reads from the PTY reader, feeds raw bytes to the vt100
    /// parser for proper terminal rendering, and also splits into lines for
    /// the line buffer used by Claude Code output analysis.
    ///
    /// The writer handle is used to respond to terminal queries such as
    /// cursor position reports (`CSI 6 n`), which many programs (fzf, shells)
    /// send to determine where to draw their UI.
    fn reader_thread(
        mut reader: Box<dyn Read + Send>,
        buffer: Arc<Mutex<Vec<String>>>,
        buffer_limit: Arc<Mutex<usize>>,
        screen: Arc<Mutex<vt100::Parser>>,
        last_output_time: Arc<Mutex<Instant>>,
        alt_screen_entered: Arc<AtomicBool>,
        writer: Arc<Mutex<Box<dyn Write + Send>>>,
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

                    // Update the last output timestamp.
                    {
                        let mut t = last_output_time.lock().unwrap_or_else(|e| e.into_inner());
                        *t = Instant::now();
                    }

                    // Count terminal queries that need responses BEFORE
                    // feeding to the parser (the parser consumes the bytes).
                    let cpr_count = count_csi_dsr(bytes);

                    // Feed raw bytes to vt100 for proper rendering.
                    {
                        let mut parser = screen.lock().unwrap_or_else(|e| e.into_inner());
                        parser.process(bytes);

                        // Respond to Cursor Position Report requests (CSI 6 n).
                        // Programs like fzf, zsh, and bash send this to
                        // determine the current cursor position for inline
                        // rendering.  Without a response, they block until a
                        // timeout or until the user types something.
                        if cpr_count > 0 {
                            let cursor = parser.screen().cursor_position();
                            // Terminal coordinates are 1-based.
                            let response = format!(
                                "\x1b[{};{}R",
                                cursor.0 + 1,
                                cursor.1 + 1,
                            );
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
                            log::debug!("ALT_SCREEN reader: entered alternate screen, chunk_size={n}");
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
                        let line = line.trim_end_matches('\n').trim_end_matches('\r').to_string();
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

    /// Push a single line into the shared buffer, enforcing the current limit.
    fn push_line(
        buffer: &Arc<Mutex<Vec<String>>>,
        buffer_limit: &Arc<Mutex<usize>>,
        line: String,
    ) {
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

// ---------------------------------------------------------------------------
// Free helper functions
// ---------------------------------------------------------------------------

/// Count the number of Cursor Position Report requests (`CSI 6 n` = `\x1b[6n`)
/// in a byte slice.  Programs send this to ask the terminal "where is the
/// cursor?" and expect a `CSI row ; col R` response.
fn count_csi_dsr(bytes: &[u8]) -> usize {
    if bytes.len() < 4 {
        return 0;
    }
    bytes
        .windows(4)
        .filter(|w| *w == b"\x1b[6n")
        .count()
}

