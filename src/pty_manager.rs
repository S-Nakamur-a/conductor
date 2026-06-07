//! PTY session management.
//!
//! Uses `portable-pty` to spawn and manage pseudo-terminal sessions so that
//! users can run shell commands or Claude Code directly inside the TUI.
//!
//! Each session is backed by a real pseudo-terminal, with a background reader
//! thread that captures output into a bounded line buffer.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use uuid::Uuid;

/// Maximum number of raw PTY output bytes retained per session for
/// reflow-on-resize. When the terminal width changes, vt100 cannot reflow
/// existing content, so the parser is rebuilt by replaying this byte history
/// at the new width. The cap is sized to comfortably exceed the vt100
/// scrollback (a few thousand lines), so eviction only ever drops content
/// already scrolled out of reach. Old bytes are trimmed at line boundaries.
const MAX_RAW_HISTORY_BYTES: usize = 2 * 1024 * 1024;

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
    /// A one-shot external editor (`$VISUAL` / `$EDITOR`) launched on a single
    /// file. Unlike the persistent Claude/Shell panels this is transient: it
    /// lives only while the user edits, and is torn down when the editor process
    /// exits. Excluded from the Claude-output scanner (waiting/active detection).
    Editor,
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
    /// Append-only (bounded) history of raw PTY output bytes, shared with the
    /// reader thread. Used to rebuild the vt100 parser at a new width on
    /// resize, since vt100 itself does not reflow existing content. Always
    /// accessed while holding the `screen` lock so appends stay atomic with
    /// `parser.process` and consistent with `resize_session`'s rebuild.
    raw_history: Arc<Mutex<VecDeque<u8>>>,

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
    /// Flag set by reader threads when new PTY output arrives.
    /// The main loop checks this to skip poll timeouts and render immediately.
    output_notify: Arc<AtomicBool>,
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
            output_notify: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Check and clear the PTY output notification flag.
    ///
    /// Returns `true` if any reader thread has produced new output since the
    /// last call.  Used by the main loop to skip poll timeouts and render
    /// PTY changes immediately.
    pub fn take_output_notify(&self) -> bool {
        self.output_notify.swap(false, Ordering::Relaxed)
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
    /// * `session_name` — if `Some`, pass `--name <name>` to the Claude CLI.
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
        session_name: Option<&str>,
    ) -> Result<usize> {
        // Build the command depending on the session kind, then hand off to the
        // shared spawn path.
        let cmd = match kind {
            SessionKind::ClaudeCode => {
                let mut c = CommandBuilder::new("claude");
                if let Some(resume_id) = resume_session_id {
                    c.arg("--resume");
                    c.arg(resume_id);
                }
                if let Some(name) = session_name {
                    c.arg("--name");
                    c.arg(name);
                }
                // Let the conductor MCP server find the review database.
                let db_path = repo_root.join(".conductor").join("conductor.db");
                c.env("CONDUCTOR_DB_PATH", db_path);
                c
            }
            SessionKind::Shell => CommandBuilder::new(shell_path),
            SessionKind::Editor => {
                unreachable!("editor sessions are spawned via spawn_editor_session")
            }
        };
        self.finish_spawn(kind, worktree, label, working_dir, rows, cols, cmd)
    }

    /// Spawn an external editor (`$VISUAL` / `$EDITOR`) on a single `file` as a
    /// transient PTY session. `program` + `args` is the resolved editor command
    /// line (already split into program and arguments); `file` is appended as
    /// the final argument.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_editor_session(
        &mut self,
        worktree: &str,
        label: &str,
        working_dir: &PathBuf,
        rows: u16,
        cols: u16,
        program: &str,
        args: &[String],
        file: &Path,
    ) -> Result<usize> {
        let mut cmd = CommandBuilder::new(program);
        for a in args {
            cmd.arg(a);
        }
        cmd.arg(file);

        // Ensure the editor sees a UTF-8 locale. `CommandBuilder` inherits the
        // parent environment, so when Conductor is launched without a UTF-8
        // locale (a bare login shell, cron, an SSH session forwarding `LANG=C`,
        // …) the editor inherits that too — and terminal editors like vim then
        // fall back to `encoding=latin1`, mangling full-width / multi-byte input.
        let (locale_sets, locale_removes) = utf8_locale_overrides(
            std::env::var("LC_ALL").ok().as_deref(),
            std::env::var("LC_CTYPE").ok().as_deref(),
            std::env::var("LANG").ok().as_deref(),
        );
        for (key, value) in &locale_sets {
            cmd.env(key, value);
        }
        for key in &locale_removes {
            cmd.env_remove(key);
        }

        self.finish_spawn(SessionKind::Editor, worktree, label, working_dir, rows, cols, cmd)
    }

    /// Shared tail of the spawn path: open the PTY pair, wire the reader thread
    /// and vt100 parser, and push the session. `cmd` is the fully built command
    /// (its working directory is set here).
    #[allow(clippy::too_many_arguments)]
    fn finish_spawn(
        &mut self,
        kind: SessionKind,
        worktree: &str,
        label: &str,
        working_dir: &PathBuf,
        rows: u16,
        cols: u16,
        mut cmd: CommandBuilder,
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
        let screen: Arc<Mutex<vt100::Parser>> = Arc::new(Mutex::new(vt100::Parser::new(
            rows,
            cols,
            self.inactive_scrollback,
        )));

        // 5c. Raw byte history for reflow-on-resize.
        let raw_history: Arc<Mutex<VecDeque<u8>>> = Arc::new(Mutex::new(VecDeque::new()));

        // 6. Spawn a background thread that continuously reads PTY output.
        let buffer_clone = Arc::clone(&output_buffer);
        let screen_clone = Arc::clone(&screen);
        let raw_history_clone = Arc::clone(&raw_history);
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

        let output_notify_for_thread = Arc::clone(&self.output_notify);

        thread::Builder::new()
            .name(format!("pty-reader-{label}"))
            .spawn(move || {
                Self::reader_thread(
                    reader,
                    buffer_clone,
                    buffer_limit_for_thread,
                    screen_clone,
                    raw_history_clone,
                    last_output_time_for_thread,
                    alt_screen_entered_for_thread,
                    writer_for_thread,
                    output_notify_for_thread,
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
            raw_history,
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
        let mut writer = session.writer.lock().unwrap_or_else(|e| e.into_inner());
        writer.write_all(data).context("Failed to write to PTY")?;
        writer.flush().context("Failed to flush PTY writer")?;
        Ok(())
    }

    /// Translate a mouse-wheel scroll into arrow-key presses for a session
    /// that is showing the **alternate screen** (e.g. pagers like `less`,
    /// `bat`, `man`).
    ///
    /// The alternate screen has no scrollback of its own, so the local
    /// scrollback offset is meaningless there — the user must scroll the
    /// child program instead. This mirrors the "alternate-scroll" behavior
    /// of tmux / iTerm2: each wheel notch becomes `lines` Up/Down arrow
    /// presses sent to the child.
    ///
    /// Returns `true` if the scroll was handled by injecting arrow keys, in
    /// which case the caller must **not** also adjust the local scrollback
    /// offset. Returns `false` when the session is not on the alternate
    /// screen, or when the child has enabled mouse reporting (in which case
    /// it captures the wheel itself and synthesizing arrows would fight it).
    pub fn scroll_alt_screen_session(&mut self, idx: usize, lines: usize, up: bool) -> bool {
        // Read the relevant terminal modes, then drop the session/parser
        // borrow before writing (write_to_session needs &mut self).
        let (is_alt, app_cursor, mouse_on) = {
            let Some(session) = self.sessions.get(idx) else {
                return false;
            };
            let parser = session.screen.lock().unwrap_or_else(|e| e.into_inner());
            let screen = parser.screen();
            (
                screen.alternate_screen(),
                screen.application_cursor(),
                screen.mouse_protocol_mode() != vt100::MouseProtocolMode::None,
            )
        };

        if !is_alt || mouse_on {
            return false;
        }

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
        let mut writer = session.writer.lock().unwrap_or_else(|e| e.into_inner());

        // Begin bracketed paste mode.
        writer
            .write_all(b"\x1b[200~")
            .context("Failed to write paste-start to PTY")?;
        writer.flush().context("Failed to flush PTY writer")?;

        // Write the payload in small chunks. Split on UTF-8 character
        // boundaries so a flushed chunk never ends with a truncated multi-byte
        // sequence (see `utf8_chunks`) — otherwise full-width / multi-byte text
        // split across the 1 KiB boundary can be mis-decoded by the receiver.
        for chunk in utf8_chunks(text, CHUNK_SIZE) {
            writer
                .write_all(chunk.as_bytes())
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

    /// Get the vt100 screen parser for the session at the given index.
    ///
    /// Returns a clone of the `Arc` so the UI can lock it for rendering.
    pub fn get_screen(&self, idx: usize) -> Option<Arc<Mutex<vt100::Parser>>> {
        self.sessions.get(idx).map(|s| Arc::clone(&s.screen))
    }

    /// Resize both the real PTY and the vt100 parser for the session at `idx`.
    ///
    /// Returns `true` when the vt100 parser was rebuilt by replaying the raw
    /// byte history (i.e. content was reflowed at a new width). A rows-only
    /// change returns `false`, as vt100 handles that without losing wrapping.
    ///
    /// vt100's `set_size` does not reflow: on a column change it clears each
    /// row's wrap flag and truncates/pads rows in place, so previously wrapped
    /// lines stay wrapped at the old width. To make old content follow the new
    /// width, we rebuild the parser from the recorded raw byte stream, which
    /// re-wraps as it is re-parsed.
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

        if old_cols == cols {
            // Width unchanged — only the row count differs. vt100 handles this
            // correctly in place, no reflow needed.
            parser.set_size(rows, cols);
            return false;
        }

        // Width changed — rebuild the parser at the new width by replaying the
        // raw byte history. Holding the `screen` lock keeps this consistent
        // with the reader thread, which appends to `raw_history` and processes
        // into the parser under the same lock.
        let history = session
            .raw_history
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *parser = Self::rebuild_parser(&history, rows, cols, scrollback);
        true
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
    #[allow(clippy::too_many_arguments)]
    fn reader_thread(
        mut reader: Box<dyn Read + Send>,
        buffer: Arc<Mutex<Vec<String>>>,
        buffer_limit: Arc<Mutex<usize>>,
        screen: Arc<Mutex<vt100::Parser>>,
        raw_history: Arc<Mutex<VecDeque<u8>>>,
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

                        // Record the same bytes for reflow-on-resize. Done under
                        // the `screen` lock so the recorded stream stays exactly
                        // in sync with what the parser has processed, and so a
                        // concurrent `resize_session` rebuild sees a consistent
                        // history. The inner scope releases the history guard
                        // before the CPR / alt-screen work below.
                        {
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
    fn trim_raw_history(history: &mut VecDeque<u8>, cap: usize) {
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

    /// Build a fresh vt100 parser of the given size by replaying the recorded
    /// raw byte history, re-wrapping content at the new width. This is the core
    /// of `resize_session`'s reflow path, factored out so it can be unit-tested
    /// without spawning a real PTY.
    fn rebuild_parser(
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

// ---------------------------------------------------------------------------
// Free helper functions
// ---------------------------------------------------------------------------

/// Decide the locale-environment overrides needed so a spawned editor treats its
/// I/O as UTF-8, given the inherited `LC_ALL` / `LC_CTYPE` / `LANG` values.
///
/// Terminal editors derive their character encoding from the locale: vim, for
/// instance, falls back to `encoding=latin1` when no UTF-8 locale is active,
/// which garbles full-width / multi-byte (e.g. Japanese) text on input *and*
/// when reading the file back.
///
/// Returns `(sets, removes)`: environment variables to set, and variables to
/// remove, on the child command. When a UTF-8 locale is already active the
/// user's setting is respected and both lists are empty.
fn utf8_locale_overrides(
    lc_all: Option<&str>,
    lc_ctype: Option<&str>,
    lang: Option<&str>,
) -> (Vec<(&'static str, &'static str)>, Vec<&'static str>) {
    fn denotes_utf8(value: &str) -> bool {
        let value = value.to_ascii_lowercase();
        value.contains("utf-8") || value.contains("utf8")
    }
    // An empty value (`LANG=`) is equivalent to the variable being unset.
    fn active(value: Option<&str>) -> Option<&str> {
        value.filter(|s| !s.is_empty())
    }

    // POSIX precedence: LC_ALL overrides LC_CTYPE, which overrides LANG.
    let effective = active(lc_all).or(active(lc_ctype)).or(active(lang));
    if effective.is_some_and(denotes_utf8) {
        return (Vec::new(), Vec::new());
    }

    // `C.UTF-8` is a locale-neutral UTF-8 locale: it exists on modern Linux and
    // is parsed by vim into `encoding=utf-8` on macOS even though it is not a
    // separately installed locale there. We set only `LC_CTYPE` (the category
    // that governs character encoding) to avoid changing the editor's message
    // language. A non-UTF-8 `LC_ALL` would shadow that, so drop it when present.
    let mut removes = Vec::new();
    if active(lc_all).is_some() {
        removes.push("LC_ALL");
    }
    (vec![("LC_CTYPE", "C.UTF-8")], removes)
}

/// Split `text` into consecutive sub-slices each at most `max` **bytes** long,
/// never cutting through a multi-byte UTF-8 character.
///
/// The PTY writers chunk large payloads (with a flush, and a small delay at the
/// chunk limit, between chunks) to stay under the kernel's PTY input buffer.
/// Splitting the raw byte slice at a fixed offset can land in the middle of a
/// multi-byte character; the receiving application then sees a truncated
/// sequence and may render a replacement / garbage glyph. Backing each split off
/// to the nearest character boundary keeps full-width / multi-byte input intact.
fn utf8_chunks(text: &str, max: usize) -> Vec<&str> {
    assert!(max > 0, "chunk size must be positive");
    let mut chunks = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let mut end = max.min(rest.len());
        // Back off until `end` lands on a character boundary (it always does at
        // `rest.len()`, so this terminates).
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        // Defensive: only reachable if `max` is smaller than the first
        // character (never the case for the 1 KiB chunk size used here). Emit
        // that whole character so we always make forward progress.
        if end == 0 {
            end = rest.chars().next().map_or(rest.len(), char::len_utf8);
        }
        let (chunk, tail) = rest.split_at(end);
        chunks.push(chunk);
        rest = tail;
    }
    chunks
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

/// Return the escape sequence for an Up/Down arrow key press used to scroll a
/// pager on the alternate screen.
///
/// `up` selects Up (`true`) vs Down (`false`). `app_cursor` honors DECCKM
/// (application cursor keys mode): when set, terminals send SS3 (`ESC O`)
/// sequences; otherwise CSI (`ESC [`). Pagers like `less` enable application
/// cursor mode and bind the SS3 forms, so respecting it is necessary for the
/// arrow keys to register reliably across programs.
fn scroll_arrow_sequence(up: bool, app_cursor: bool) -> &'static [u8] {
    match (up, app_cursor) {
        (true, true) => b"\x1bOA",   // Up   (SS3)
        (true, false) => b"\x1b[A",  // Up   (CSI)
        (false, true) => b"\x1bOB",  // Down (SS3)
        (false, false) => b"\x1b[B", // Down (CSI)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrow_sequence_honors_decckm_and_direction() {
        // Application cursor keys mode → SS3 (ESC O); note the letter O (0x4f).
        assert_eq!(scroll_arrow_sequence(true, true), b"\x1bOA");
        assert_eq!(scroll_arrow_sequence(false, true), b"\x1bOB");
        // Normal mode → CSI (ESC [).
        assert_eq!(scroll_arrow_sequence(true, false), b"\x1b[A");
        assert_eq!(scroll_arrow_sequence(false, false), b"\x1b[B");
    }

    #[test]
    fn trim_raw_history_keeps_under_cap_and_aligns_to_line() {
        let mut history: VecDeque<u8> = b"aaaa\nbbbb\ncccc\ndddd\n".iter().copied().collect();
        // Cap below current length forces a trim.
        PtyManager::trim_raw_history(&mut history, 10);
        let remaining: Vec<u8> = history.iter().copied().collect();
        // Must be at or under the cap...
        assert!(remaining.len() <= 10, "len={}", remaining.len());
        // ...and resume at a clean line boundary (no leading partial line).
        let text = String::from_utf8(remaining).unwrap();
        for line in text.split_inclusive('\n') {
            if line.ends_with('\n') {
                // Every retained complete line is one of the originals.
                assert!(["aaaa\n", "bbbb\n", "cccc\n", "dddd\n"].contains(&line));
            }
        }
        // The oldest content ("aaaa") must have been dropped.
        assert!(!text.contains("aaaa"));
    }

    #[test]
    fn trim_raw_history_noop_when_within_cap() {
        let mut history: VecDeque<u8> = b"hello\n".iter().copied().collect();
        PtyManager::trim_raw_history(&mut history, 1024);
        assert_eq!(history.iter().copied().collect::<Vec<u8>>(), b"hello\n");
    }

    #[test]
    fn trim_raw_history_noop_when_cap_equals_len() {
        let mut history: VecDeque<u8> = b"abcd\n".iter().copied().collect();
        let len = history.len();
        PtyManager::trim_raw_history(&mut history, len);
        assert_eq!(history.len(), len);
    }

    #[test]
    fn trim_raw_history_empty_is_safe() {
        let mut history: VecDeque<u8> = VecDeque::new();
        PtyManager::trim_raw_history(&mut history, 0);
        assert!(history.is_empty());
    }

    /// A buffer over cap with NO newline must NOT be drained to empty — a blank
    /// terminal on resize is far worse than a slightly imperfect first line.
    /// (This is the failure mode of escape-sequence-heavy TUI output.)
    #[test]
    fn trim_raw_history_keeps_bytes_when_no_newline() {
        let mut history: VecDeque<u8> = std::iter::repeat_n(b'x', 100).collect();
        PtyManager::trim_raw_history(&mut history, 10);
        assert!(!history.is_empty(), "history was drained to empty");
        assert!(history.len() <= 10);
        assert!(history.iter().all(|&b| b == b'x'));
    }

    /// Replaying the raw byte stream into a fresh parser at a new width must
    /// reflow content that originally wrapped at the old width — this is the
    /// core of `resize_session`'s column-change path. (vt100's own `set_size`
    /// does not reflow, which is the bug this whole change fixes.)
    #[test]
    fn replay_reflows_to_new_width() {
        // A single 12-char logical line, no explicit newline.
        let stream = b"ABCDEFGHIJKL";

        // Narrow parser (cols=4): the line wraps across 3 physical rows, but
        // vt100 tracks the wrap so it is still one logical line.
        let mut narrow = vt100::Parser::new(10, 4, 100);
        narrow.process(stream);
        assert_eq!(narrow.screen().contents().trim_end(), "ABCDEFGHIJKL");

        // vt100's set_size does NOT reflow: widening clears the wrap flags, so
        // the three physical rows become three separate logical lines instead
        // of re-joining and re-wrapping at the new width. This is the bug.
        narrow.set_size(10, 12);
        assert_eq!(
            narrow.screen().contents().trim_end(),
            "ABCD\nEFGH\nIJKL",
            "set_size unexpectedly reflowed — the bug may be fixed upstream",
        );

        // Replaying the same stream via rebuild_parser at the wide width
        // reflows correctly: the line fits on one row.
        let history: VecDeque<u8> = stream.iter().copied().collect();
        let wide = PtyManager::rebuild_parser(&history, 10, 12, 100);
        assert_eq!(wide.screen().contents().trim_end(), "ABCDEFGHIJKL");
    }

    #[test]
    fn rebuild_parser_handles_empty_history() {
        let history: VecDeque<u8> = VecDeque::new();
        let parser = PtyManager::rebuild_parser(&history, 5, 20, 100);
        assert_eq!(parser.screen().contents().trim_end(), "");
    }

    /// Replaying a stream that enters and exits the alternate screen must
    /// reconstruct the correct final (normal-screen) state, since alt-screen
    /// transitions are pure byte sequences in the stream.
    #[test]
    fn rebuild_parser_reconstructs_alt_screen_roundtrip() {
        // normal text, enter alt screen, draw, exit alt screen, more normal text
        let mut stream: Vec<u8> = Vec::new();
        stream.extend_from_slice(b"normal-before\r\n");
        stream.extend_from_slice(b"\x1b[?1049h"); // enter alt screen
        stream.extend_from_slice(b"ALT-CONTENT");
        stream.extend_from_slice(b"\x1b[?1049l"); // exit alt screen
        stream.extend_from_slice(b"normal-after");

        let history: VecDeque<u8> = stream.iter().copied().collect();
        let parser = PtyManager::rebuild_parser(&history, 6, 40, 100);

        // Back on the normal screen after the roundtrip.
        assert!(!parser.screen().alternate_screen());
        let contents = parser.screen().contents();
        assert!(contents.contains("normal-before"), "got: {contents:?}");
        assert!(contents.contains("normal-after"), "got: {contents:?}");
        // Alt-screen content does not bleed into the normal grid.
        assert!(!contents.contains("ALT-CONTENT"), "got: {contents:?}");
    }

    #[test]
    fn utf8_chunks_never_splits_a_multibyte_char() {
        // Each kana is 3 bytes in UTF-8. With max=4, a naive byte split would
        // cut the second character; utf8_chunks must keep every char intact.
        let text = "あいうえお"; // 5 chars × 3 bytes = 15 bytes
        let chunks = utf8_chunks(text, 4);
        // Reassembling the chunks must reproduce the input exactly...
        assert_eq!(chunks.concat(), text);
        // ...and every chunk must be valid (one whole 3-byte char fits in 4).
        for chunk in &chunks {
            assert_eq!(chunk.chars().count(), 1);
            assert!(chunk.len() <= 4);
        }
    }

    #[test]
    fn utf8_chunks_preserves_mixed_and_ascii_text() {
        let text = "abc日本語def";
        // A chunk size that lands mid-character on a naive split.
        let chunks = utf8_chunks(text, 5);
        assert_eq!(chunks.concat(), text);
        for chunk in &chunks {
            assert!(chunk.len() <= 5);
            // No chunk boundary fell inside a character: re-parsing is lossless.
            assert!(std::str::from_utf8(chunk.as_bytes()).is_ok());
        }
    }

    #[test]
    fn utf8_chunks_handles_char_wider_than_max() {
        // Defensive path: a single char larger than `max` is emitted whole
        // rather than looping forever.
        let chunks = utf8_chunks("あ", 1); // 'あ' is 3 bytes
        assert_eq!(chunks, vec!["あ"]);
    }

    #[test]
    fn utf8_chunks_empty_input_yields_no_chunks() {
        assert!(utf8_chunks("", 1024).is_empty());
    }

    // ── utf8_locale_overrides ────────────────────────────────────────────

    #[test]
    fn locale_unset_everywhere_forces_utf8() {
        // The load-bearing case: a stripped environment makes vim default to
        // latin1, which garbles full-width input. We inject a UTF-8 LC_CTYPE.
        let (sets, removes) = utf8_locale_overrides(None, None, None);
        assert_eq!(sets, vec![("LC_CTYPE", "C.UTF-8")]);
        assert!(removes.is_empty());
    }

    #[test]
    fn locale_empty_values_are_treated_as_unset() {
        // macOS commonly leaves LANG/LC_ALL empty; an empty value must not be
        // mistaken for an active non-UTF-8 locale.
        let (sets, removes) = utf8_locale_overrides(Some(""), Some(""), Some(""));
        assert_eq!(sets, vec![("LC_CTYPE", "C.UTF-8")]);
        assert!(removes.is_empty());
    }

    #[test]
    fn locale_existing_utf8_is_respected() {
        // LC_CTYPE=UTF-8 (the macOS Terminal default) already yields utf-8.
        assert_eq!(
            utf8_locale_overrides(None, Some("UTF-8"), None),
            (Vec::new(), Vec::new())
        );
        // A full UTF-8 locale in LANG is honored too.
        assert_eq!(
            utf8_locale_overrides(None, None, Some("en_US.UTF-8")),
            (Vec::new(), Vec::new())
        );
        // Case-insensitive and the `utf8` spelling both count.
        assert_eq!(
            utf8_locale_overrides(None, None, Some("ja_JP.utf8")),
            (Vec::new(), Vec::new())
        );
    }

    #[test]
    fn locale_lc_all_takes_precedence_for_detection() {
        // A UTF-8 LC_ALL wins even if LANG is a non-UTF-8 locale.
        assert_eq!(
            utf8_locale_overrides(Some("C.UTF-8"), None, Some("C")),
            (Vec::new(), Vec::new())
        );
    }

    #[test]
    fn locale_non_utf8_lc_all_is_dropped_so_lc_ctype_can_win() {
        // LC_ALL shadows LC_CTYPE, so a non-UTF-8 LC_ALL must be removed for the
        // injected LC_CTYPE to take effect.
        let (sets, removes) = utf8_locale_overrides(Some("C"), Some("C"), Some("C"));
        assert_eq!(sets, vec![("LC_CTYPE", "C.UTF-8")]);
        assert_eq!(removes, vec!["LC_ALL"]);
    }

    #[test]
    fn locale_non_utf8_lang_without_lc_all_keeps_lc_all_untouched() {
        let (sets, removes) = utf8_locale_overrides(None, None, Some("C"));
        assert_eq!(sets, vec![("LC_CTYPE", "C.UTF-8")]);
        assert!(removes.is_empty());
    }
}
