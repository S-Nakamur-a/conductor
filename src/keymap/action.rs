//! The customisable action vocabulary — the [`Action`] enum, its stable
//! config names (via the `keymap-suite` [`actions!`](keymap_suite::actions)
//! macro), human-readable labels for the cheatsheet, and the
//! terminal-interception classification that [`KeyMap`](super::KeyMap)
//! consults during resolution.

// ---------------------------------------------------------------------------
// Action — every customisable user action
// ---------------------------------------------------------------------------

// The `actions!` macro (keymap-suite 0.1.2) generates the enum, the
// `ActionName` impl (`from_name` / `name` — the config-name mapping the old
// hand-written `from_str` / `as_str` provided), and `Action::ALL` in
// declaration order (which the cheatsheet renders, so declaration order IS
// display order). Default chords are deliberately NOT declared here: they live
// in `default_keybinds.toml`, whose per-layer tables fit this app's
// nine-layer, shared-navigation keymap better than the macro's per-action
// `"chord" @ "layer"` lists would (see the file for the schema and rationale
// comments users read when overriding).
keymap_suite::actions! {
    pub enum Action {
        // ── Global ────────────────────────────────────────────────────
        Quit => "quit",
        ShowHelp => "show_help",
        CommandPalette => "command_palette",
        CycleFocusForward => "cycle_focus_forward",
        CycleFocusBackward => "cycle_focus_backward",
        /// Switch the selected worktree to the next/previous one, from any panel
        /// (the worktree strip follows the selection). Distinct from focus cycling.
        NextWorktree => "next_worktree",
        PrevWorktree => "prev_worktree",
        FocusWorktree => "focus_worktree",
        FocusExplorer => "focus_explorer",
        FocusExplorerDiffList => "focus_explorer_diff_list",
        FocusViewer => "focus_viewer",
        FocusTerminalClaude => "focus_terminal_claude",
        FocusTerminalShell => "focus_terminal_shell",
        NewClaudeCode => "new_claude_code",
        NewShell => "new_shell",
        OpenRepo => "open_repo",
        SwitchRepo => "switch_repo",

        // ── Shared navigation ────────────────────────────────────────
        NavigateUp => "navigate_up",
        NavigateDown => "navigate_down",
        GoToTop => "go_to_top",
        GoToBottom => "go_to_bottom",
        ExpandOrRight => "expand_or_right",
        CollapseOrLeft => "collapse_or_left",
        Select => "select",

        // ── Worktree panel ───────────────────────────────────────────
        CreateWorktree => "create_worktree",
        DeleteWorktree => "delete_worktree",
        SwitchBranch => "switch_branch",
        GrabBranch => "grab_branch",
        UngrabBranch => "ungrab_branch",
        PruneWorktrees => "prune_worktrees",
        MergeToMain => "merge_to_main",
        RefreshWorktrees => "refresh_worktrees",
        ResetMainToOrigin => "reset_main_to_origin",
        CherryPick => "cherry_pick",
        PullWorktree => "pull_worktree",
        SessionHistory => "session_history",
        OpenPullRequest => "open_pull_request",

        // ── Explorer panel ───────────────────────────────────────────
        ShowDiffList => "show_diff_list",
        ShowCommentList => "show_comment_list",
        /// Open the full-screen comment-list modal (overview of all comments on the
        /// branch, with jump-to-location).
        OpenCommentList => "open_comment_list",
        SearchFilename => "search_filename",
        DeleteComment => "delete_comment",
        ToggleResolve => "toggle_resolve",
        EditComment => "edit_comment",
        ReplyToComment => "reply_to_comment",
        ViewCommentDetail => "view_comment_detail",
        ExitSubPanel => "exit_sub_panel",

        // ── Viewer panel ─────────────────────────────────────────────
        ScrollHalfPageDown => "scroll_half_page_down",
        ScrollHalfPageUp => "scroll_half_page_up",
        ScrollLeft => "scroll_left",
        ScrollRight => "scroll_right",
        ScrollHome => "scroll_home",
        SearchInFile => "search_in_file",
        NextSearchMatch => "next_search_match",
        PrevSearchMatch => "prev_search_match",
        AddComment => "add_comment",
        ExitToExplorer => "exit_to_explorer",
        /// Open the file shown in the Viewer in an external editor ($VISUAL /
        /// $EDITOR): suspend the TUI, run the editor, then restore and reload.
        OpenInEditor => "open_in_editor",
        /// Switch a markdown file in the Viewer between raw source and rendered
        /// prose. No-op on any other file (and in diff mode), since the two
        /// views only differ for markdown.
        ToggleMarkdownRender => "toggle_markdown_render",

        // ── Terminal panel ────────────────────────────────────────────
        LeaveTerminal => "leave_terminal",
        ScrollbackUp => "scrollback_up",
        ScrollbackDown => "scrollback_down",
        ScrollbackTop => "scrollback_top",
        SnapToLive => "snap_to_live",
        OpenFileFromTerminal => "open_file_from_terminal",
        /// Cycle to the next/previous session tab in the focused terminal panel
        /// (Claude Code or Shell) — the keyboard equivalent of clicking a tab.
        NextSession => "next_session",
        PrevSession => "prev_session",

        // ── App ──────────────────────────────────────────────────────
        UpdateAndRestart => "update_and_restart",

        // ── Search ──────────────────────────────────────────────────
        SearchFullText => "search_full_text",

        // ── Code navigation ─────────────────────────────────────────
        JumpBack => "jump_back",
        JumpForward => "jump_forward",
        /// Show a hover popup with the type/signature, doc comment, and
        /// reference count of the symbol under the viewer cursor.
        ShowHoverInfo => "show_hover_info",
        ToggleInlineThread => "toggle_inline_thread",
        InlineReply => "inline_reply",

        // ── Diff navigation ─────────────────────────────────────────
        NextHunk => "next_hunk",
        PrevHunk => "prev_hunk",
        NextComment => "next_comment",
        PrevComment => "prev_comment",
        /// Jump to the next/previous changed file in the diff list (GitHub-style
        /// "next file" — the lightweight substitute for cross-file scrolling).
        NextChangedFile => "next_changed_file",
        PrevChangedFile => "prev_changed_file",

        // ── Diff context expansion ─────────────────────────────────
        ExpandContext => "expand_context",
        ExpandAllContext => "expand_all_context",

        // ── Panel layout ────────────────────────────────────────────
        TogglePanelExpand => "toggle_panel_expand",
        TogglePanelOverlay => "toggle_panel_overlay",
        /// Grow the focused panel toward the left (tmux `resize-pane -L`).
        ResizePaneLeft => "resize_pane_left",
        /// Grow the focused panel toward the right (tmux `resize-pane -R`).
        ResizePaneRight => "resize_pane_right",
        /// Grow the focused panel upward (tmux `resize-pane -U`).
        ResizePaneUp => "resize_pane_up",
        /// Grow the focused panel downward (tmux `resize-pane -D`).
        ResizePaneDown => "resize_pane_down",

        // ── UI ──────────────────────────────────────────────────────
        /// Open the theme picker overlay to switch the UI color theme at runtime.
        OpenThemePicker => "open_theme_picker",

        // ── PR review ───────────────────────────────────────────────
        /// Show the AI walkthrough as the Explorer's bottom-pane view.
        ShowWalkthrough => "show_walkthrough",
        /// Toggle the "viewed" mark on a file (diff list row, or the file
        /// currently open in the Viewer's diff mode).
        ToggleViewed => "toggle_viewed",
        /// Move the walkthrough step selection to the next/previous step and jump
        /// to it immediately (only in the Explorer's Walkthrough view).
        WalkthroughNextStep => "walkthrough_next_step",
        WalkthroughPrevStep => "walkthrough_prev_step",
        /// Open the PR-number/URL input overlay to fetch (or reuse) a worktree
        /// for a pull request.
        ReviewPullRequest => "review_pull_request",
        /// Generate (or regenerate) the AI walkthrough for the selected
        /// worktree's branch via a background headless Claude session.
        /// Palette-only by default: `g` is the go-to-top idiom everywhere, and a
        /// minutes-long generation is too expensive to fire from a slipped key.
        GenerateWalkthrough => "generate_walkthrough",
        /// Regenerate the walkthrough even when one already exists for the
        /// current branch tip — the escape hatch past `GenerateWalkthrough`'s
        /// same-commit skip (e.g. to pick up an improved generation prompt).
        ForceGenerateWalkthrough => "force_generate_walkthrough",
        /// Publish this branch's unpublished review comments to the GitHub PR
        /// they were opened from. Palette-only: it's an irreversible external
        /// action and always goes through a y/n confirm overlay first.
        PublishReview => "publish_review",
    }
}

impl Action {
    /// Human-readable one-line description of the action, for the cheatsheet.
    pub fn label(self) -> &'static str {
        match self {
            Action::Quit => "Quit application",
            Action::ShowHelp => "Toggle this cheatsheet",
            Action::CommandPalette => "Command palette",
            Action::CycleFocusForward => "Focus next panel",
            Action::CycleFocusBackward => "Focus previous panel",
            Action::NextWorktree => "Next worktree",
            Action::PrevWorktree => "Previous worktree",
            Action::FocusWorktree => "Open worktree switcher",
            Action::FocusExplorer => "Focus Explorer (file tree)",
            Action::FocusExplorerDiffList => "Focus Changed-files list",
            Action::FocusViewer => "Focus Viewer",
            Action::FocusTerminalClaude => "Focus Claude Code panel",
            Action::FocusTerminalShell => "Focus Shell panel",
            Action::NewClaudeCode => "New Claude Code session",
            Action::NewShell => "New Shell session",
            Action::OpenRepo => "Open repository by path",
            Action::SwitchRepo => "Switch repository",
            Action::NavigateUp => "Move up",
            Action::NavigateDown => "Move down",
            Action::GoToTop => "Jump to top",
            Action::GoToBottom => "Jump to bottom",
            Action::ExpandOrRight => "Expand / move right",
            Action::CollapseOrLeft => "Collapse / move left",
            Action::Select => "Select / open",
            Action::CreateWorktree => "Create new worktree",
            Action::DeleteWorktree => "Delete selected worktree",
            Action::SwitchBranch => "Switch (checkout) branch",
            Action::GrabBranch => "Grab branch into worktree",
            Action::UngrabBranch => "Ungrab branch",
            Action::PruneWorktrees => "Prune merged worktrees",
            Action::MergeToMain => "Merge to main",
            Action::RefreshWorktrees => "Refresh worktrees",
            Action::ResetMainToOrigin => "Reset main to origin",
            Action::CherryPick => "Cherry-pick",
            Action::PullWorktree => "Pull worktree",
            Action::SessionHistory => "Session history",
            Action::OpenPullRequest => "Open pull request",
            Action::ShowWalkthrough => "Show AI walkthrough",
            Action::ToggleViewed => "Toggle file viewed",
            Action::WalkthroughNextStep => "Jump to next walkthrough step",
            Action::WalkthroughPrevStep => "Jump to previous walkthrough step",
            Action::ReviewPullRequest => "Review a pull request by number or URL",
            Action::GenerateWalkthrough => "Generate an AI walkthrough of this branch's diff",
            Action::ForceGenerateWalkthrough => "Regenerate the walkthrough (ignore same-commit skip)",
            Action::PublishReview => "Publish unpublished review comments to the GitHub PR",
            Action::ShowDiffList => "Show changed-files list",
            Action::ShowCommentList => "Show comment list",
            Action::OpenCommentList => "Open comment-list modal",
            Action::SearchFilename => "Search filenames",
            Action::DeleteComment => "Delete comment / reply",
            Action::ToggleResolve => "Toggle resolved",
            Action::EditComment => "Edit comment / reply",
            Action::ReplyToComment => "Reply to comment",
            Action::ViewCommentDetail => "View comment detail",
            Action::ExitSubPanel => "Exit sub-panel",
            Action::ScrollHalfPageDown => "Scroll half page down",
            Action::ScrollHalfPageUp => "Scroll half page up",
            Action::ScrollLeft => "Scroll left",
            Action::ScrollRight => "Scroll right",
            Action::ScrollHome => "Scroll to line start",
            Action::SearchInFile => "Search in file",
            Action::NextSearchMatch => "Next search match",
            Action::PrevSearchMatch => "Previous search match",
            Action::AddComment => "Add comment on line",
            Action::ExitToExplorer => "Back to Explorer",
            Action::OpenInEditor => "Open in $EDITOR",
            Action::ToggleMarkdownRender => "Toggle markdown Raw / Rendered",
            Action::LeaveTerminal => "Leave terminal (keep session)",
            Action::ScrollbackUp => "Scrollback up",
            Action::ScrollbackDown => "Scrollback down",
            Action::ScrollbackTop => "Scrollback to top",
            Action::SnapToLive => "Snap to live output",
            Action::OpenFileFromTerminal => "Open file from terminal output",
            Action::NextSession => "Next session tab",
            Action::PrevSession => "Previous session tab",
            Action::UpdateAndRestart => "Update and restart",
            Action::SearchFullText => "Full-text search (grep)",
            Action::JumpBack => "Jump back (history)",
            Action::JumpForward => "Jump forward (history)",
            Action::ShowHoverInfo => "Show hover info (signature/doc)",
            Action::ToggleInlineThread => "Toggle inline comment thread",
            Action::InlineReply => "Inline reply",
            Action::NextHunk => "Next hunk",
            Action::PrevHunk => "Previous hunk",
            Action::NextComment => "Next comment",
            Action::PrevComment => "Previous comment",
            Action::NextChangedFile => "Next changed file",
            Action::PrevChangedFile => "Previous changed file",
            Action::ExpandContext => "Expand diff context",
            Action::ExpandAllContext => "Expand all diff context",
            Action::TogglePanelExpand => "Maximize / restore panel",
            Action::TogglePanelOverlay => "Toggle panel-number overlay",
            Action::ResizePaneLeft => "Resize pane left",
            Action::ResizePaneRight => "Resize pane right",
            Action::ResizePaneUp => "Resize pane up",
            Action::ResizePaneDown => "Resize pane down",
            Action::OpenThemePicker => "Open theme picker",
        }
    }

    /// Whether this action is intercepted while a terminal panel (PTY) is
    /// focused. `false` (the default) means the chord is forwarded to the inner
    /// program (shell / Claude Code), so Conductor never steals a key the
    /// program needs — `ctrl+r` reverse-search, `ctrl+q`/XON, etc. Only the
    /// focus/navigation/scrollback actions listed here are stolen back. This is
    /// the single source of truth for terminal interception: both
    /// [`KeyMap::resolve`](super::KeyMap::resolve) and
    /// [`KeyMap::keys_for_action`](super::KeyMap::keys_for_action) honor it, so
    /// resolution == behavior == the rendered help, with no hand-maintained
    /// allowlist in the dispatcher.
    pub(crate) fn fires_in_terminal(self) -> bool {
        matches!(
            self,
            // Terminal-only actions (meaningful only with a terminal focused).
            Action::LeaveTerminal
                | Action::ScrollbackUp
                | Action::ScrollbackDown
                | Action::ScrollbackTop
                | Action::SnapToLive
                | Action::OpenFileFromTerminal
                // Cycling session tabs is a terminal-panel action by definition.
                | Action::NextSession
                | Action::PrevSession
                // Global focus/navigation that stays useful over a PTY.
                | Action::FocusWorktree
                | Action::FocusExplorer
                | Action::FocusExplorerDiffList
                | Action::FocusViewer
                | Action::FocusTerminalClaude
                | Action::FocusTerminalShell
                | Action::CommandPalette
                | Action::CycleFocusForward
                | Action::CycleFocusBackward
                | Action::NextWorktree
                | Action::PrevWorktree
                | Action::TogglePanelExpand
                | Action::TogglePanelOverlay
                // Pane resizing is most useful with a terminal focused (resize
                // the Claude/Shell split or the terminal column while typing in
                // it), so these must fire over a PTY too.
                | Action::ResizePaneLeft
                | Action::ResizePaneRight
                | Action::ResizePaneUp
                | Action::ResizePaneDown
        )
    }
}
