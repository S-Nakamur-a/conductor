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
    /// Last applied content area size (rows, cols) for the embedded editor PTY.
    /// Tracked separately so `sync_pty_sizes` only resizes on an actual change.
    pub size_editor: (u16, u16),
    /// Scrollback offset for the Claude Code terminal (0 = live view).
    pub scroll_claude: usize,
    /// Scrollback offset for the Shell terminal (0 = live view).
    pub scroll_shell: usize,
    /// Cached PTY render output for Claude terminal.
    pub cache_claude: PtyRenderCache,
    /// Cached PTY render output for Shell terminal.
    pub cache_shell: PtyRenderCache,
    /// Worktree paths whose Claude Code sessions are actively working.
    pub cc_active_worktrees: HashSet<PathBuf>,
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
    /// Set when PTY reader thread produces new output for Claude terminal.
    pub dirty_claude: bool,
    /// Set when PTY reader thread produces new output for Shell terminal.
    pub dirty_shell: bool,
    /// First-visible tab index for the Claude session tab strip (horizontal scroll).
    pub claude_tab_scroll: usize,
    /// First-visible tab index for the Shell session tab strip (horizontal scroll).
    pub shell_tab_scroll: usize,
    /// Pan the Claude tab strip to reveal the active tab on the next render.
    pub claude_tab_reveal: bool,
    /// Pan the Shell tab strip to reveal the active tab on the next render.
    pub shell_tab_reveal: bool,
    /// Clickable regions of the Claude tab strip, recorded each render.
    pub claude_tab_hits: Vec<crate::ui::tab_bar::TabHit>,
    /// Clickable regions of the Shell tab strip, recorded each render.
    pub shell_tab_hits: Vec<crate::ui::tab_bar::TabHit>,
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
            size_editor: (24, 80),
            scroll_claude: 0,
            scroll_shell: 0,
            cache_claude: Default::default(),
            cache_shell: Default::default(),
            cc_active_worktrees: HashSet::new(),
            cc_waiting_worktrees: HashSet::new(),
            cc_waiting_ack_time: HashMap::new(),
            claude_blank_last_click: Instant::now(),
            shell_blank_last_click: Instant::now(),
            needs_clear: false,
            deferred_prompts: HashMap::new(),
            dirty_claude: true,
            dirty_shell: true,
            claude_tab_scroll: 0,
            shell_tab_scroll: 0,
            claude_tab_reveal: false,
            shell_tab_reveal: false,
            claude_tab_hits: Vec::new(),
            shell_tab_hits: Vec::new(),
        }
    }

    /// Switch the Claude panel to display the session at `idx`.
    ///
    /// Marks the PTY session active, records it as the active Claude session,
    /// and resets the panel's scroll offset and render cache. Clearing the
    /// cache is essential: it is a single buffer shared across sessions, so
    /// without this the panel would keep rendering the previous session's
    /// content until some other trigger (scroll, new output) happened to
    /// rebuild it. See `ui::terminal_claude` for the rebuild condition.
    pub fn switch_claude_session(&mut self, idx: usize) {
        self.pty_manager.activate_session(idx);
        self.active_claude_session = Some(idx);
        self.scroll_claude = 0;
        self.cache_claude = PtyRenderCache::default();
        self.claude_tab_reveal = true;
    }

    /// Switch the Shell panel to display the session at `idx`.
    ///
    /// Shell-side counterpart of [`Self::switch_claude_session`]; same cache
    /// invalidation rationale applies.
    pub fn switch_shell_session(&mut self, idx: usize) {
        self.pty_manager.activate_session(idx);
        self.active_shell_session = Some(idx);
        self.scroll_shell = 0;
        self.cache_shell = PtyRenderCache::default();
        self.shell_tab_reveal = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::text::Line;

    /// Seed a render cache so we can prove `switch_*` clears it. Mirrors the
    /// stale state that caused the panel-not-syncing bug: a non-empty cache
    /// left over from a previously displayed session.
    fn stale_cache() -> PtyRenderCache {
        PtyRenderCache {
            lines: vec![Line::from("previous session output")],
            effective_offset: 7,
            cursor_position: Some((3, 4)),
        }
    }

    #[test]
    fn switch_claude_session_resets_scroll_and_cache() {
        let mut term = TerminalState::new(1000, 100);
        term.scroll_claude = 42;
        term.cache_claude = stale_cache();

        // No sessions exist; activate_session is a tolerant no-op, so this
        // exercises the panel-state reset in isolation.
        term.switch_claude_session(0);

        assert_eq!(term.active_claude_session, Some(0));
        assert_eq!(term.scroll_claude, 0);
        assert!(
            term.cache_claude.lines.is_empty(),
            "cache must be cleared so the render guard rebuilds for the new session"
        );
        assert_eq!(term.cache_claude.effective_offset, 0);
    }

    #[test]
    fn switch_shell_session_resets_scroll_and_cache() {
        let mut term = TerminalState::new(1000, 100);
        term.scroll_shell = 42;
        term.cache_shell = stale_cache();

        term.switch_shell_session(2);

        assert_eq!(term.active_shell_session, Some(2));
        assert_eq!(term.scroll_shell, 0);
        assert!(term.cache_shell.lines.is_empty());
        assert_eq!(term.cache_shell.effective_offset, 0);
    }

    #[test]
    fn switch_claude_session_leaves_shell_panel_untouched() {
        let mut term = TerminalState::new(1000, 100);
        term.scroll_shell = 5;
        term.cache_shell = stale_cache();

        term.switch_claude_session(0);

        // Switching the Claude panel must not disturb the Shell panel's state.
        assert_eq!(term.scroll_shell, 5);
        assert!(!term.cache_shell.lines.is_empty());
    }
}
