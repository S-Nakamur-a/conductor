//! カスタマイズ可能なアクションの語彙。enum と安定した設定名は actions! マクロが
//! 1 つの宣言から生成する。既定のチョードは default_keybinds.toml にある。

keymap_suite::actions! {
    pub enum Action {
        // グローバル
        Quit => "quit",
        ShowHelp => "show_help",
        CommandPalette => "command_palette",
        FocusMenuBar => "focus_menu_bar",
        CycleFocusForward => "cycle_focus_forward",
        CycleFocusBackward => "cycle_focus_backward",
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

        // 共通ナビゲーション
        NavigateUp => "navigate_up",
        NavigateDown => "navigate_down",
        GoToTop => "go_to_top",
        GoToBottom => "go_to_bottom",
        ExpandOrRight => "expand_or_right",
        CollapseOrLeft => "collapse_or_left",
        Select => "select",

        // ワークツリーパネル
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

        // エクスプローラパネル
        ShowDiffList => "show_diff_list",
        ShowCommentList => "show_comment_list",
        OpenCommentList => "open_comment_list",
        SearchFilename => "search_filename",
        DeleteComment => "delete_comment",
        ToggleResolve => "toggle_resolve",
        EditComment => "edit_comment",
        ReplyToComment => "reply_to_comment",
        ViewCommentDetail => "view_comment_detail",
        ExitSubPanel => "exit_sub_panel",

        // ビューアパネル
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
        OpenInEditor => "open_in_editor",
        NextViewerTab => "next_viewer_tab",
        PrevViewerTab => "prev_viewer_tab",
        CloseViewerTab => "close_viewer_tab",
        ToggleMarkdownRender => "toggle_markdown_render",
        ToggleDiffView => "toggle_diff_view",

        // ターミナルパネル
        LeaveTerminal => "leave_terminal",
        ScrollbackUp => "scrollback_up",
        ScrollbackDown => "scrollback_down",
        ScrollbackTop => "scrollback_top",
        SnapToLive => "snap_to_live",
        OpenFileFromTerminal => "open_file_from_terminal",
        NextSession => "next_session",
        PrevSession => "prev_session",

        // 検索
        SearchFullText => "search_full_text",

        // コードナビゲーション
        JumpBack => "jump_back",
        JumpForward => "jump_forward",
        ShowHoverInfo => "show_hover_info",
        ToggleInlineThread => "toggle_inline_thread",
        InlineReply => "inline_reply",

        // 差分ナビゲーション
        NextHunk => "next_hunk",
        PrevHunk => "prev_hunk",
        NextComment => "next_comment",
        PrevComment => "prev_comment",
        NextChangedFile => "next_changed_file",
        PrevChangedFile => "prev_changed_file",

        // 差分コンテキストの展開
        ExpandContext => "expand_context",
        ExpandAllContext => "expand_all_context",

        // パネルレイアウト
        TogglePanelExpand => "toggle_panel_expand",
        ResizePaneLeft => "resize_pane_left",
        ResizePaneRight => "resize_pane_right",
        ResizePaneUp => "resize_pane_up",
        ResizePaneDown => "resize_pane_down",
        OpenThemePicker => "open_theme_picker",

        // PR レビュー
        ShowRevidere => "show_revidere",
        RevidereNextSection => "revidere_next_section",
        RevidererPrevSection => "revidere_prev_section",
        /// 総括と節+diff は 1 キーで交互に切り替えず、行き先ごとにキーを分ける。
        /// 押した結果がいまどちらを出しているかに依存しないようにするため。
        RevidereShowOverview => "revidere_show_overview",
        RevidereShowSections => "revidere_show_sections",
        RevidereToggleScope => "revidere_toggle_scope",
        ToggleViewed => "toggle_viewed",
        /// 'z' の 2 打鍵目 (za/zc/zo/zm/zr/zR/zM) はハンドラ側が直接読む。
        /// gd/gi/gr と同じ扱いで、折りたたみだけが再割り当て可能な語彙を持つ理由がない。
        FoldPrefix => "fold_prefix",
        ReviewPullRequest => "review_pull_request",
        AnalyzeRevidere => "analyze_revidere",
        ForceAnalyzeRevidere => "force_analyze_revidere",
        /// パレット限定。取り消せない外部アクションなので、常に確認を先に通す。
        PublishReview => "publish_review",
    }
}

impl Action {
    /// チートシート向けの 1 行説明。
    pub fn label(self) -> &'static str {
        match self {
            Action::Quit => "Quit application",
            Action::ShowHelp => "Toggle this cheatsheet",
            Action::CommandPalette => "Command palette",
            Action::FocusMenuBar => "Focus the menu bar",
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
            Action::SessionHistory => "Saved terminal output",
            Action::OpenPullRequest => "Open pull request",
            Action::ShowRevidere => "Show the review (sections + diff)",
            Action::RevidereNextSection => "Jump to next section",
            Action::RevidererPrevSection => "Jump to previous section",
            Action::RevidereShowOverview => "Show the overview (1 column)",
            Action::RevidereShowSections => "Show the sections + diff (2 columns)",
            Action::RevidereToggleScope => {
                "Switch between the whole branch and what changed since the last review"
            }
            Action::AnalyzeRevidere => "Review this branch (asks first)",
            Action::ForceAnalyzeRevidere => "Re-analyse without asking, ignoring the cached reply",
            Action::FoldPrefix => "Fold block (za/zc/zo/zm/zr/zR/zM)",
            Action::ToggleViewed => "Toggle file viewed",
            Action::ReviewPullRequest => "Review a pull request by number or URL",
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
            Action::NextViewerTab => "Next file tab",
            Action::PrevViewerTab => "Previous file tab",
            Action::CloseViewerTab => "Close file tab",
            Action::ToggleDiffView => "Toggle unified / side-by-side diff",
            Action::ToggleMarkdownRender => "Toggle markdown Raw / Rendered",
            Action::LeaveTerminal => "Leave terminal (keep session)",
            Action::ScrollbackUp => "Scrollback up",
            Action::ScrollbackDown => "Scrollback down",
            Action::ScrollbackTop => "Scrollback to top",
            Action::SnapToLive => "Snap to live output",
            Action::OpenFileFromTerminal => "Open file from terminal output",
            Action::NextSession => "Next session tab",
            Action::PrevSession => "Previous session tab",
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
            Action::ResizePaneLeft => "Resize pane left",
            Action::ResizePaneRight => "Resize pane right",
            Action::ResizePaneUp => "Resize pane up",
            Action::ResizePaneDown => "Resize pane down",
            Action::OpenThemePicker => "Open theme picker",
        }
    }

    /// PTY を持つパネルがフォーカスされている間、このアクションを横取りするか。
    /// false のチョードは内側のプログラムへ転送される (ctrl+r や ctrl+q/XON を奪わない)。
    /// PTY 横取りの唯一の真実の源で、KeyMap::resolve も keys_for_action もこれに従う。
    pub fn fires_in_terminal(self) -> bool {
        matches!(
            self,
            Action::LeaveTerminal
                | Action::ScrollbackUp
                | Action::ScrollbackDown
                | Action::ScrollbackTop
                | Action::SnapToLive
                | Action::OpenFileFromTerminal
                | Action::NextSession
                | Action::PrevSession
                | Action::FocusWorktree
                | Action::FocusExplorer
                | Action::FocusExplorerDiffList
                | Action::FocusViewer
                | Action::FocusTerminalClaude
                | Action::FocusTerminalShell
                | Action::CommandPalette
                | Action::FocusMenuBar
                | Action::CycleFocusForward
                | Action::CycleFocusBackward
                | Action::NextWorktree
                | Action::PrevWorktree
                | Action::TogglePanelExpand
                | Action::ResizePaneLeft
                | Action::ResizePaneRight
                | Action::ResizePaneUp
                | Action::ResizePaneDown
        )
    }
}
