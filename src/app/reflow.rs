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
    /// Resolves the session backing the currently displayed Claude panel (via
    /// its pinned `claude_session_id`), loads and parses that `.jsonl` file, and
    /// activates the overlay. Falls back to the selected worktree's most recent
    /// session only when the panel has no tracked id. If no session is found or
    /// the log is empty, a status flash is shown and the view stays inactive.
    pub fn open_reflow(&mut self) {
        // Prefer the session backing the *currently displayed* Claude panel. A
        // worktree can host several Claude panels (CC:1, CC:2, …), so resolving
        // "the worktree's latest session" would open whichever log was written
        // most recently regardless of which panel is on screen — that is the
        // cross-panel scroll bleed this view used to suffer from. The pinned
        // per-session id (see `PtySession::claude_session_id`) ties the
        // transcript to the panel the user is actually looking at.
        let resolved = self
            .terminal
            .active_claude_session
            .and_then(|idx| self.terminal.pty_manager.claude_session_ref(idx));

        // Working dir of the selected worktree, used for the mtime fallback.
        let working_dir = match self.worktrees.get(self.selected_worktree) {
            Some(wt) => wt.path.clone(),
            None => {
                self.set_status(
                    "No worktree selected for transcript view".to_string(),
                    StatusLevel::Warning,
                );
                return;
            }
        };

        // Parse the log on a background thread: `load_session` reads and
        // JSON-parses the whole `.jsonl`, which for large sessions (5MB+)
        // would otherwise block the 60fps loop for several frames. The view
        // activates immediately with a "Loading…" placeholder and
        // `poll_reflow_load` swaps the entries in when they arrive.
        self.bg.reflow_load.start(move |tx| {
            // 1. Prefer the active panel's pinned session id. When a worktree
            //    hosts several Claude panels (CC:1, CC:2, …) this ties the
            //    transcript to the panel actually on screen, avoiding
            //    cross-panel scroll bleed.
            let pinned_path = resolved
                .and_then(|(wd, sid)| crate::claude_sessions::session_jsonl_path(&wd, &sid));
            let mut entries = pinned_path
                .as_ref()
                .map(|path| crate::claude_log::load_session(path))
                .unwrap_or_default();

            // 1b. Follow a mid-session `/clear` (or in-app `/resume`). Claude
            //     Code mints a *new* session file on `/clear`, so the pinned
            //     file freezes with the pre-clear conversation while the live
            //     turns land in a different log — scrolling up would otherwise
            //     open onto the stale pre-clear transcript. Prefer the freshest
            //     log in the project dir that is a *temporal continuation* of
            //     the pinned session: one whose first turn is at/after the
            //     pinned session's last turn. Because a concurrently-active
            //     sibling panel overlaps the pinned session in time, its first
            //     turn predates the pinned session's last turn and it is never
            //     mistaken for the continuation — the per-panel pin (and its
            //     cross-panel-bleed guarantee) is preserved.
            if let Some(pinned) = pinned_path.as_ref()
                && !entries.is_empty()
                && let Some(pinned_last) = crate::claude_log::session_last_timestamp(pinned)
            {
                let live = crate::claude_sessions::session_logs_by_mtime(&working_dir)
                    .into_iter()
                    .filter(|p| p.as_path() != pinned.as_path())
                    .find(|p| {
                        crate::claude_log::session_first_timestamp(p)
                            .is_some_and(|first| first >= pinned_last)
                    });
                if let Some(live_path) = live {
                    let live_entries = crate::claude_log::load_session(&live_path);
                    if !live_entries.is_empty() {
                        entries = live_entries;
                    }
                }
            }

            // 2. Fall back to the most-recently-written log in this worktree's
            //    project dir when the pinned session is missing or empty. This
            //    is what catches a manual in-app `/resume` sent before any turn
            //    was typed: it switches the live session id away from the
            //    conductor-launched (pinned) one, so the pinned file is
            //    stale/empty while the real transcript is whatever Claude is now
            //    appending to (= freshest mtime). Empty/aux logs (e.g. one-shot
            //    security-review runs sharing the dir) are skipped.
            if entries.is_empty() {
                entries = crate::claude_sessions::session_logs_by_mtime(&working_dir)
                    .iter()
                    .map(|path| crate::claude_log::load_session(path))
                    .find(|e| !e.is_empty())
                    .unwrap_or_default();
            }

            let _ = tx.send(entries);
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
