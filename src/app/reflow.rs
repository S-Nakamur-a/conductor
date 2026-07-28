//! Reflow transcript view: a read-only, word-wrapped session-log viewer that
//! overlays the Claude PTY panel during infinite-scrollback mode.

use super::{App, StatusLevel};

/// Active entry animation for the reflow transcript view.
///
/// Holds the `Instant` the animation started so that `reflow_view::render` can
/// compute progress via elapsed time without any frame-counter dependency.
/// Only the *entry* transition is animated — it masks the initial
/// `build_lines` latency. Leaving the view swaps back to the live PTY
/// immediately (no exit animation) so returning to the prompt feels instant.
pub struct Sweep {
    pub start: std::time::Instant,
}

/// State for the reflow transcript view.
///
/// When `active` is `true`, this view overlays the Claude PTY panel and renders
/// the Claude Code session log as a scrollable, word-wrapped Markdown display.
/// Width changes trigger a full re-render of `cached_lines`; otherwise the
/// cached lines are reused each frame.
#[derive(Default)]
pub struct ReflowView {
    /// Whether the reflow view is currently overlaying the Claude PTY panel.
    pub active: bool,
    /// Whether the session log is still being parsed on a background thread.
    /// While `true`, the view renders a "Loading…" placeholder instead of
    /// transcript lines; cleared when `poll_reflow_load` receives the entries.
    pub loading: bool,
    /// Parsed and normalised log entries from the session file.
    ///
    /// Wrapped in `Rc` so that `build_lines` can cheaply clone the handle
    /// (refcount increment only) and release its borrow on `self` before
    /// calling `cache.render` — avoiding a deep copy of all entry strings on
    /// every resize.
    pub entries: std::rc::Rc<Vec<crate::claude_log::LogEntry>>,
    /// Vertical scroll offset — number of rendered lines from the top to skip.
    pub scroll: usize,
    /// Total number of lines in `cached_lines` (kept in sync after each render).
    pub total_lines: usize,
    /// Panel inner width at the last render — used to detect size changes for reflow.
    pub last_width: u16,
    /// When `true`, the next render pins scroll to the bottom (most recent turn).
    pub pending_bottom: bool,
    /// Pre-rendered, width-reflowed lines; rebuilt only when `last_width` changes.
    pub cached_lines: Vec<ratatui::text::Line<'static>>,
    /// Inner panel height at the last render — used for page-scroll sizing.
    pub last_inner_height: u16,
    /// Per-session Markdown render cache.
    ///
    /// Kept separate from `App::markdown_cache` so it does not pollute the
    /// shared cache with reflow keys and is automatically invalidated when a new
    /// session is opened (the whole `ReflowView` is replaced by `open_reflow`).
    pub cache: crate::ui::markdown::MarkdownCache,
    /// In-progress entry/exit sweep animation, or `None` when idle.
    ///
    /// `Option<Sweep>` defaults to `None` — `Sweep` itself does not need
    /// `Default` because `Option<T>: Default` is always `None` without a
    /// `T: Default` bound.
    pub sweep: Option<Sweep>,
}

impl App {
    // ── Reflow transcript view ────────────────────────────────────────────

    /// Enter the reflow transcript view for the active Claude panel's session.
    ///
    /// Resolves the log of the session backing the currently displayed Claude
    /// panel — strictly by its pinned `claude_session_id` — then loads and parses
    /// that `.jsonl` and activates the overlay. When the id does not resolve to a
    /// log, or the log holds no displayable turn, a status flash explains why and
    /// the view stays inactive; no other session's history is ever substituted.
    pub fn open_reflow(&mut self) {
        // The transcript source is the session backing the *currently displayed*
        // Claude panel, identified only by its pinned session id (see
        // `PtySession::claude_session_id`).
        //
        // Nothing here may widen to a directory-level criterion. One Claude
        // project dir holds the logs of every session ever run in that worktree
        // — sibling Conductor panels (CC:1, CC:2, …), earlier runs, plain
        // `claude` invocations — so "the freshest log" or "the log whose first
        // turn follows this one's last turn" can name a different conversation.
        // The latter is how this view used to show the wrong session's history:
        // it treated a later-starting log as the continuation of a `/clear`
        // rotation, which held only while the pinned session kept writing turns.
        // A panel whose main agent is stopped while a subagent still works
        // writes nothing to its session log (subagent turns go to
        // `<session-id>/subagents/*.jsonl`), so its last turn froze and any
        // session started later in the same worktree passed the test and
        // hijacked the view for as long as the subagent ran.
        //
        // Consequence worth knowing: a mid-session `/clear` or an in-app
        // `/resume` rotates Claude Code's live log to a new session id that
        // Conductor cannot observe, so scrolling up then shows this session's
        // pre-rotation transcript. Stale-but-own beats fresh-but-someone-else's.
        let Some((working_dir, session_id)) = self
            .terminal
            .active_claude_session
            .and_then(|idx| self.terminal.pty_manager.claude_session_ref(idx))
        else {
            self.set_status(
                "No Claude session for this panel; transcript unavailable".to_string(),
                StatusLevel::Warning,
            );
            return;
        };

        let Some(path) = crate::claude_sessions::session_jsonl_path(&working_dir, &session_id)
        else {
            self.set_status(
                format!("No session log on disk for {session_id}"),
                StatusLevel::Warning,
            );
            return;
        };

        // Parse the log on a background thread: `load_session` reads and
        // JSON-parses the whole `.jsonl`, which for large sessions (5MB+)
        // would otherwise block the 60fps loop for several frames. The view
        // activates immediately with a "Loading…" placeholder and
        // `poll_reflow_load` swaps the entries in when they arrive.
        self.bg.reflow_load.start(move |tx| {
            let _ = tx.send(crate::claude_log::load_session(&path));
        });

        self.reflow = ReflowView {
            active: true,
            loading: true,
            entries: std::rc::Rc::new(Vec::new()),
            scroll: 0,
            total_lines: 0,
            last_width: 0, // Forces a full line rebuild on first render.
            pending_bottom: true,
            cached_lines: Vec::new(),
            last_inner_height: 0,
            cache: crate::ui::markdown::MarkdownCache::new(),
            // Start the entry transition: the border glides from the accent to
            // its complement over TRANSITION_DURATION_MS, masking the initial
            // load + build_lines latency.
            sweep: Some(Sweep {
                start: std::time::Instant::now(),
            }),
        };
    }

    /// Apply a finished background session-log parse to the reflow view.
    ///
    /// Discards the result when the view was closed while loading (stale). An
    /// empty log closes the view with a status flash — the same outcome the
    /// old synchronous path produced before activating the view.
    pub fn poll_reflow_load(&mut self) {
        let Some(entries) = self.bg.reflow_load.poll() else {
            return;
        };
        if !self.reflow.active {
            return; // View closed while loading; drop the stale result.
        }
        if entries.is_empty() {
            self.close_reflow();
            self.set_status(
                "Session log is empty or unreadable".to_string(),
                StatusLevel::Info,
            );
            return;
        }
        self.reflow.entries = std::rc::Rc::new(entries);
        self.reflow.loading = false;
        self.reflow.last_width = 0; // Force a full line rebuild on next render.
        self.reflow.pending_bottom = true;
        // The entry sweep may already have finished on a slow load; redraw so
        // the transcript replaces the "Loading…" placeholder immediately.
        self.dirty.mark(super::DirtyPanels::TERMINAL);
    }

    /// Leave the reflow transcript view and return to the live PTY display.
    pub fn close_reflow(&mut self) {
        self.reflow.active = false;
        self.reflow.sweep = None;
        // Cancel any in-flight background log parse so a stale result can't
        // arrive after the view is gone (or leak into the next open).
        self.bg.reflow_load.clear();
        // Reset Claude scrollback so the live tail is shown immediately.
        self.terminal.scroll_claude = 0;
        // Force a fresh PTY snapshot on the next frame. While the reflow view
        // was up the PTY panel rendered nothing, so `cache_claude` holds the
        // pre-scrollback frame. If no new output happens to arrive right after
        // closing (e.g. Claude is idle at its prompt), the stale cache would
        // otherwise persist and the input box would not reappear. Clearing the
        // cache and marking it dirty rebuilds the live tail immediately.
        self.terminal.cache_claude = Default::default();
        self.terminal.dirty_claude = true;
    }

    /// Leave the reflow transcript view, returning to the live PTY immediately.
    ///
    /// Kept as a distinct entry point from `close_reflow` for the keybind/scroll
    /// call sites, but there is no exit animation: the content swaps back to the
    /// live tail on the same frame so returning to the prompt feels instant.
    pub fn request_close_reflow(&mut self) {
        self.close_reflow();
    }
}
