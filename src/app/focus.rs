//! Panel focus model: which panel has keyboard focus, the focus-cycle order,
//! and the border-color glide animation driven by focus changes.

use super::App;

/// Which panel currently has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Worktree,
    Explorer,
    Viewer,
    TerminalClaude,
    TerminalShell,
    /// The embedded editor panel (vim/emacs in a PTY) occupying the merged
    /// Explorer+Viewer region. Only reachable while [`App::editor`] is `Some`.
    Editor,
}

impl Focus {
    /// The base keymap context for this panel. Both terminals share the
    /// `Terminal` context (sub-modes like diff/comment lists are tracked
    /// separately by the panels themselves).
    pub fn key_context(self) -> crate::keymap::KeyContext {
        use crate::keymap::KeyContext;
        match self {
            Focus::Worktree => KeyContext::Worktree,
            Focus::Explorer => KeyContext::Explorer,
            Focus::Viewer => KeyContext::Viewer,
            Focus::TerminalClaude | Focus::TerminalShell => KeyContext::Terminal,
            Focus::Editor => KeyContext::Editor,
        }
    }

    /// Whether this panel hosts a PTY whose inner program (Claude Code, shell,
    /// or an editor) should receive raw keystrokes. The event dispatcher routes
    /// these panels through the PTY-forwarding path; the keymap only steals back
    /// the chords that [fire in the terminal](crate::keymap::Action).
    pub fn is_pty(self) -> bool {
        matches!(
            self,
            Focus::TerminalClaude | Focus::TerminalShell | Focus::Editor
        )
    }
}

impl App {
    /// Set focus to a panel, lazily loading data when first needed.
    pub fn set_focus(&mut self, mut focus: Focus) {
        // While the embedded editor occupies the merged Explorer+Viewer region,
        // those two panels are hidden — any request to focus them lands on the
        // editor instead. Centralizing the redirect here keeps every focus path
        // (Tab cycle, alt+digit, click, palette) honoring the invariant without
        // each needing to know about the editor.
        if self.editor.is_some() && matches!(focus, Focus::Explorer | Focus::Viewer) {
            focus = Focus::Editor;
        }

        // The worktree column became a monitor strip + switcher modal, so
        // "focus the worktree" now opens that modal and leaves focus where it
        // was. This is the single chokepoint every worktree trigger funnels
        // through (Tab no longer reaches Worktree, super+1/`w`/palette/click all
        // call set_focus(Worktree)).
        if focus == Focus::Worktree {
            self.overlays.active = crate::overlay::ActiveOverlay::WorktreeSwitcher;
            return;
        }

        // Collapse expanded panel when focus moves to a panel that would have zero width.
        if let Some(expanded) = self.expanded_panel {
            let dominated = match expanded {
                Focus::TerminalClaude | Focus::TerminalShell => {
                    matches!(focus, Focus::TerminalClaude | Focus::TerminalShell)
                }
                other => other == focus,
            };
            if !dominated {
                self.expanded_panel = None;
            }
        }
        // Note: we deliberately do NOT close the reflow transcript here on a
        // plain focus change. Both the key handler (`event`) and the renderer
        // (`ui::terminal_claude`) gate reflow on `focus == TerminalClaude`, so
        // while another panel is focused the transcript neither captures keys
        // nor renders (the Claude panel falls back to the live PTY). Tearing it
        // down here would also reset the scroll offset, snapping the user back to
        // the live tail when they merely glanced at another panel. Reflow is
        // still closed on the transitions where the transcript becomes stale —
        // session switch (`switch_claude_session`) and worktree change
        // (`on_worktree_changed`) — and by Esc/F4 in the reflow key handler.

        match focus {
            Focus::Explorer | Focus::Viewer => {
                if self.viewer_state.tree.file_tree.is_empty() {
                    self.refresh_viewer();
                }
                if self.diff_state.committed_files.is_empty()
                    && self.diff_state.uncommitted_files.is_empty()
                {
                    self.refresh_diff();
                }
            }
            Focus::TerminalClaude => {
                // Clear CC waiting signal when user focuses on the terminal panel,
                // not just when they actually type into it.
                if let Some(idx) = self.terminal.active_claude_session {
                    self.clear_cc_waiting_signal(idx);
                }
            }
            _ => {}
        }
        // A panel's transient search prompt is modal to that panel; moving focus
        // away must release key capture. Otherwise the search box keeps eating
        // keystrokes after focus moves (e.g. `/` in the viewer, then Tab to
        // Claude — input should go to Claude). The query/matches persist so n/N
        // still work when you return.
        if focus != Focus::Viewer {
            self.viewer_state.search.search_active = false;
        }
        if matches!(
            focus,
            Focus::TerminalClaude | Focus::TerminalShell | Focus::Editor
        ) {
            self.viewer_state.filename_search.filename_search_active = false;
        }
        // Record the change so the gaining/losing panels can glide their border
        // color (only on an actual change, so a re-focus doesn't restart it).
        if self.focus != focus {
            self.focus_prev = self.focus;
            self.focus_changed_at = std::time::Instant::now();
        }
        self.focus = focus;
    }

    /// Border color for `panel`, eased across focus changes: the panel gaining
    /// focus glides `border_unfocused → border_focused`, the one losing it
    /// glides back, over `anim::FOCUS_MS`. Everything else rests on the static
    /// unfocused color. This is what makes panel switches feel smooth instead of
    /// snapping, using the theme's RGB colors and `Theme::lerp`.
    pub fn animated_border_color(&self, panel: Focus) -> ratatui::style::Color {
        let t = crate::anim::eased_progress(self.focus_changed_at.elapsed(), crate::anim::FOCUS_MS);
        if self.focus == panel {
            if t >= 1.0 {
                self.theme.border_focused
            } else {
                crate::theme::Theme::lerp(self.theme.border_unfocused, self.theme.border_focused, t)
            }
        } else if self.focus_prev == panel && t < 1.0 {
            crate::theme::Theme::lerp(self.theme.border_focused, self.theme.border_unfocused, t)
        } else {
            self.theme.border_unfocused
        }
    }

    /// Whether a UI transition (currently the focus-border glide) is still in
    /// flight. The main loop uses this to keep redrawing at the active frame
    /// rate so the transition actually animates instead of stalling at the idle
    /// tick rate.
    pub fn has_active_transition(&self) -> bool {
        self.focus_changed_at.elapsed() < std::time::Duration::from_millis(crate::anim::FOCUS_MS)
    }

    // ── Focus cycling ────────────────────────────────────────────────

    /// Cycle focus forward: Worktree → Explorer → Viewer → TerminalClaude → TerminalShell → Worktree
    pub fn cycle_focus_forward(&mut self) {
        // Worktree is no longer a focusable column (it became the top strip +
        // switcher modal), so it's excluded from the Tab cycle.
        // When the editor is open it stands in for Explorer+Viewer in the cycle;
        // `set_focus` redirects any Explorer/Viewer target onto it, so the only
        // explicit arm needed is leaving the editor itself.
        //
        // The Explorer column holds two independent panels — the file tree and
        // the changed-files list — so Tab visits each as its own stop (file tree
        // → changed files → Viewer), toggling the sub-focus before moving on.
        if self.editor.is_none()
            && self.focus == Focus::Explorer
            && !self.viewer_state.explorer.explorer_focus_on_diff_list
        {
            self.viewer_state.explorer.explorer_focus_on_diff_list = true;
            self.focus_changed_at = std::time::Instant::now();
            return;
        }
        let next = match self.focus {
            Focus::Worktree | Focus::TerminalShell => Focus::Explorer,
            Focus::Explorer => Focus::Viewer,
            Focus::Viewer => Focus::TerminalClaude,
            Focus::Editor => Focus::TerminalClaude,
            Focus::TerminalClaude => Focus::TerminalShell,
        };
        // Landing on the Explorer column from elsewhere always starts on the
        // file tree (the top panel).
        if next == Focus::Explorer {
            self.viewer_state.explorer.explorer_focus_on_diff_list = false;
        }
        self.set_focus(next);
    }

    /// Cycle focus backward.
    pub fn cycle_focus_backward(&mut self) {
        // Mirror of the forward cycle: stepping back through the Explorer column
        // visits changed files then the file tree.
        if self.editor.is_none()
            && self.focus == Focus::Explorer
            && self.viewer_state.explorer.explorer_focus_on_diff_list
        {
            self.viewer_state.explorer.explorer_focus_on_diff_list = false;
            self.focus_changed_at = std::time::Instant::now();
            return;
        }
        let prev = match self.focus {
            Focus::Worktree | Focus::Explorer => Focus::TerminalShell,
            Focus::Viewer => Focus::Explorer,
            Focus::Editor => Focus::TerminalShell,
            Focus::TerminalClaude => Focus::Viewer,
            Focus::TerminalShell => Focus::TerminalClaude,
        };
        // Entering the Explorer column from the Viewer side lands on the
        // changed-files panel (nearest), so a further Tab-back reaches the tree.
        if prev == Focus::Explorer {
            self.viewer_state.explorer.explorer_focus_on_diff_list = true;
        }
        self.set_focus(prev);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_is_pty_only_for_pty_panels() {
        assert!(Focus::TerminalClaude.is_pty());
        assert!(Focus::TerminalShell.is_pty());
        assert!(Focus::Editor.is_pty());
        assert!(!Focus::Worktree.is_pty());
        assert!(!Focus::Explorer.is_pty());
        assert!(!Focus::Viewer.is_pty());
    }

    #[test]
    fn editor_focus_uses_editor_keymap_context() {
        assert_eq!(Focus::Editor.key_context(), crate::keymap::KeyContext::Editor);
    }
}
