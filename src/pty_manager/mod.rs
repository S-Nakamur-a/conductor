//! PTY session management.
//!
//! Uses `portable-pty` to spawn and manage pseudo-terminal sessions so that
//! users can run shell commands or Claude Code directly inside the TUI.
//!
//! Each session is backed by a real pseudo-terminal, with a background reader
//! thread that captures output into a bounded line buffer.
//!
//! This module holds the `PtySession`/`PtyManager` types and the small
//! lifecycle methods (construction, activation, removal). The rest of the
//! behavior is split by responsibility into submodules:
//! [`spawn`] (launching Claude/Shell/Editor processes), [`io`] (writing input
//! and forwarding scroll events), [`screen`] (vt100 access, resize/reflow,
//! input-waiting detection), [`reader`] (the background reader thread), and
//! [`locale`] (UTF-8 locale/chunking helpers).

use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result};
use portable_pty::NativePtySystem;

mod io;
mod locale;
mod reader;
mod screen;
mod spawn;
#[cfg(test)]
mod tests;

/// Maximum number of raw PTY output bytes retained per session for
/// reflow-on-resize. When the terminal width changes, vt100 cannot reflow
/// existing content, so the parser is rebuilt by replaying this byte history
/// at the new width.
///
/// This replay runs synchronously on the main thread inside `resize_session`,
/// so its cost is paid as a UI stall on *every* width change — panel maximize,
/// focus shifts that resize the shared right column, Tab, tmux-style resize.
/// The cap is therefore kept modest: 512 KiB still covers well over the
/// default active scrollback (10 000 lines at typical shell line lengths) while
/// keeping a worst-case replay to a single-frame stall instead of tens of ms.
/// Bytes beyond the cap are trimmed at line boundaries — the only content lost
/// to reflow is history already far out of view.
const MAX_RAW_HISTORY_BYTES: usize = 512 * 1024;

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
    /// The Claude Code session id (`<id>.jsonl` under the project dir) backing
    /// this panel, when known. Set for `ClaudeCode` sessions: a fresh spawn
    /// forces a generated id via `--session-id`, and a resumed spawn records the
    /// id it resumed. `None` for Shell/Editor sessions, and for any Claude
    /// session whose id could not be determined. Lets the reflow transcript view
    /// open *this* panel's log instead of merely the worktree's latest session —
    /// essential when one worktree hosts multiple Claude panels (CC:1, CC:2, …).
    pub claude_session_id: Option<String>,
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
    ///
    /// `None` for sessions that cannot benefit from replay-based reflow.
    /// Replay only re-wraps content that relied on terminal autowrap (soft
    /// wrapping) — e.g. ordinary shell output. In-place-repaint apps like
    /// Claude Code lay out every line with absolute cursor-column escapes and
    /// hard line breaks baked at the current width, so replaying their bytes at
    /// a new width reproduces the identical old-width layout — no reflow, just
    /// wasted memory and CPU. Those sessions skip recording entirely.
    raw_history: Option<Arc<Mutex<VecDeque<u8>>>>,

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

    /// Return the number of sessions.
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// The Claude session id and working directory for the session at `idx`,
    /// when it is a Claude panel with a known id. Used by the reflow transcript
    /// view to open the log for a *specific* panel rather than the worktree's
    /// most recently written session. Returns `None` for out-of-range indices,
    /// non-Claude sessions, or Claude sessions whose id is unknown.
    pub fn claude_session_ref(&self, idx: usize) -> Option<(PathBuf, String)> {
        let session = self.sessions.get(idx)?;
        let id = session.claude_session_id.as_ref()?;
        Some((session.working_dir.clone(), id.clone()))
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
}
