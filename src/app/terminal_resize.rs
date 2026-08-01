//! PTY size syncing and diff/viewer staleness polling for [`App`].
//!
//! Keeps Claude/Shell/editor PTY grids sized to their rendered panel area,
//! and the lightweight per-tick check that decides whether the diff and
//! viewer panels need refreshing.

use super::*;

impl App {
    /// Sync PTY session sizes with cached layout dimensions.
    /// Only resizes when dimensions actually changed.
    pub fn sync_pty_sizes(
        &mut self,
        last_claude_size: &mut (u16, u16),
        last_shell_size: &mut (u16, u16),
    ) {
        let cols = &self.layout.cache.columns;
        let is_terminal_expanded = matches!(
            self.expanded_panel,
            Some(crate::app::Focus::TerminalClaude | crate::app::Focus::TerminalShell)
        );
        let border_cols: u16 = if is_terminal_expanded { 0 } else { 2 };
        let border_rows: u16 = if is_terminal_expanded { 1 } else { 2 };
        let right_w = cols[3].width;
        if right_w > border_cols {
            let right_cols = right_w.saturating_sub(border_cols);
            let claude_pty_rows = self.layout.cache.terminal_split[0]
                .height
                .saturating_sub(border_rows);
            let shell_pty_rows = self.layout.cache.terminal_split[1]
                .height
                .saturating_sub(border_rows);

            if (claude_pty_rows, right_cols) != *last_claude_size
                && claude_pty_rows > 0
                && right_cols > 0
            {
                *last_claude_size = (claude_pty_rows, right_cols);
                self.update_claude_terminal_size(claude_pty_rows, right_cols);
            }
            if (shell_pty_rows, right_cols) != *last_shell_size
                && shell_pty_rows > 0
                && right_cols > 0
            {
                *last_shell_size = (shell_pty_rows, right_cols);
                self.update_shell_terminal_size(shell_pty_rows, right_cols);
            }
        }

        // Keep the embedded editor PTY sized to its (merged Explorer+Viewer)
        // region. Computed from the cached layout, so it tracks panel resizes
        // and the maximize toggle.
        if let Some(idx) = self.editor.as_ref().map(|e| e.session_idx) {
            let size = self.editor_pty_size();
            if size != self.terminal.size_editor && size.0 > 0 && size.1 > 0 {
                self.terminal.size_editor = size;
                self.terminal
                    .pty_manager
                    .resize_session(idx, size.0, size.1);
            }
        }
    }

    /// Update the terminal content area size for Claude PTY sessions and resize them.
    pub fn update_claude_terminal_size(&mut self, rows: u16, cols: u16) {
        self.terminal.size_claude = (rows, cols);
        if self.resize_sessions_of_kind(pty_manager::SessionKind::ClaudeCode, rows, cols) {
            // The grid was rebuilt at a new width, so the cached render is stale.
            //
            // The *scroll offset* is deliberately left alone. This used to
            // reset it to 0, which snapped a reader who had scrolled back to
            // the live tail on every width change — and width changes are not
            // rare events here: a window resize, a panel maximize, a divider
            // drag, even moving focus between panels (column widths are
            // focus-driven) all reach this path. Losing your place because you
            // glanced at another window was the whole complaint.
            //
            // Keeping the number is approximate — re-wrapping renumbers the
            // rows above the viewport, so the view can shift by a few lines —
            // but it lands near where the reader was instead of at the far end
            // of the history. Anchoring it exactly, the way the transcript view
            // does with `LineMeta`, would mean probing `vt100::Parser::
            // set_scrollback` across candidate offsets, and that API underflows
            // (`Grid::visible_rows`, vt100 0.15.2) for any offset past one
            // screenful — it only survives today because release builds wrap on
            // overflow. See the note in `docs/pty-reflow-design.md`.
            self.terminal.cache_claude = Default::default();
            self.terminal.dirty_claude = true;
        }
    }

    /// Update the terminal content area size for Shell PTY sessions and resize them.
    pub fn update_shell_terminal_size(&mut self, rows: u16, cols: u16) {
        self.terminal.size_shell = (rows, cols);
        if self.resize_sessions_of_kind(pty_manager::SessionKind::Shell, rows, cols) {
            // Same as the Claude panel above: invalidate the render cache, keep
            // the reader's scroll offset. The shell is where this actually
            // fires today — only shell sessions record the `raw_history` that
            // makes `resize_session` report a reflow at all.
            self.terminal.cache_shell = Default::default();
            self.terminal.dirty_shell = true;
        }
    }

    /// Resize every session of `kind` for the selected worktree to (rows, cols).
    /// Returns `true` if any session reflowed (a width change rebuilt its grid).
    fn resize_sessions_of_kind(
        &mut self,
        kind: pty_manager::SessionKind,
        rows: u16,
        cols: u16,
    ) -> bool {
        let wt_path = self.selected_worktree_path();
        let count = self.terminal.pty_manager.session_count();
        let mut reflowed = false;
        for idx in 0..count {
            let s = &self.terminal.pty_manager.sessions()[idx];
            if s.working_dir == wt_path && s.kind == kind {
                reflowed |= self.terminal.pty_manager.resize_session(idx, rows, cols);
            }
        }
        reflowed
    }

    // ── Lightweight change-detection polling ─────────────────────────────

    /// Check whether the diff and viewer panels need refreshing by comparing
    /// the current worktree's HEAD oid and status counts against the last
    /// known values.  Only triggers the expensive `refresh_diff()` and
    /// `refresh_viewer()` when an actual change is detected.
    ///
    /// Called after `refresh_worktrees()` in the polling loop, which already
    /// fetches HEAD oids and status counts as a side effect.
    pub fn check_diff_viewer_staleness(&mut self) {
        let wt = match self.worktrees.selected() {
            Some(wt) => wt,
            None => return,
        };

        let current_head = self.worktree_heads.get(&wt.branch).cloned();
        // `staged` is here specifically so `git add` / `git reset` are visible.
        // The other three count one bucket per file with the index checked
        // first, so staging a modified file leaves all of them unchanged — and
        // the file watcher can't help either, since it ignores `.git/` and
        // staging touches nothing else. Without this component the Explorer's
        // stage-state colours (D6) would only ever update when some unrelated
        // edit happened to trigger a refresh.
        let current_status = (wt.added, wt.modified, wt.deleted, wt.staged);

        let head_changed = self.last_poll_head_oid.as_ref() != current_head.as_ref();
        let status_changed = self.last_poll_status != Some(current_status);

        if head_changed || status_changed {
            log::debug!(
                "Change detected for worktree '{}': head_changed={}, status_changed={}",
                wt.branch,
                head_changed,
                status_changed,
            );
            self.refresh_diff();
            self.refresh_viewer();
        }

        self.last_poll_head_oid = current_head;
        self.last_poll_status = Some(current_status);
    }
}
