//! Terminal / PTY state management.
//!
//! Groups all PTY-related fields previously scattered in `App` into a
//! single `TerminalState` struct.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;

use crate::pty_manager;
use crate::ui::common::PtyRenderCache;

/// Aggregated state for the dual terminal panels (Claude Code + Shell).
pub struct TerminalState {
    /// PTY session manager.
    pub pty_manager: pty_manager::PtyManager,
    /// Index of the active Claude Code session for the current worktree.
    pub active_claude_session: Option<usize>,
    /// Index of the active Shell session for the current worktree.
    pub active_shell_session: Option<usize>,
    /// Last known terminal content area size (rows, cols) for Claude PTY.
    pub size_claude: (u16, u16),
    /// Last known terminal content area size (rows, cols) for Shell PTY.
    pub size_shell: (u16, u16),
    /// Scrollback offset for the Claude Code terminal (0 = live view).
    pub scroll_claude: usize,
    /// Scrollback offset for the Shell terminal (0 = live view).
    pub scroll_shell: usize,
    /// Cached PTY render output for Claude terminal.
    pub cache_claude: PtyRenderCache,
    /// Cached PTY render output for Shell terminal.
    pub cache_shell: PtyRenderCache,
    /// Worktree paths whose Claude Code sessions are waiting for user input.
    pub cc_waiting_worktrees: HashSet<PathBuf>,
    /// Acknowledged waiting states — maps worktree path to the PTY session's
    /// `last_output_time` at the moment the user dismissed the notification.
    pub cc_waiting_ack_time: HashMap<PathBuf, Instant>,
    /// Timestamp of last click on Claude terminal blank area (for double-click detection).
    pub claude_blank_last_click: Instant,
    /// Timestamp of last click on Shell terminal blank area (for double-click detection).
    pub shell_blank_last_click: Instant,
    /// Set to `true` when a full terminal clear + redraw is needed.
    pub needs_clear: bool,
    /// Deferred prompts: session index → prompt text.
    /// Written once the CC session becomes ready (waiting for input).
    pub deferred_prompts: HashMap<usize, String>,
}

impl TerminalState {
    /// Create a new `TerminalState` with the given scrollback limits.
    pub fn new(active_scrollback: usize, inactive_scrollback: usize) -> Self {
        Self {
            pty_manager: pty_manager::PtyManager::new(active_scrollback, inactive_scrollback),
            active_claude_session: None,
            active_shell_session: None,
            size_claude: (24, 80),
            size_shell: (6, 80),
            scroll_claude: 0,
            scroll_shell: 0,
            cache_claude: Default::default(),
            cache_shell: Default::default(),
            cc_waiting_worktrees: HashSet::new(),
            cc_waiting_ack_time: HashMap::new(),
            claude_blank_last_click: Instant::now(),
            shell_blank_last_click: Instant::now(),
            needs_clear: false,
            deferred_prompts: HashMap::new(),
        }
    }
}
