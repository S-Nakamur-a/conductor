//! カスタマイズ可能なアクションの語彙 — Action enum、（keymap-suite の
//! actions!（keymap_suite::actions）マクロによる）その安定した設定名、
//! チートシート向けの人間が読めるラベル、そして KeyMap（super::KeyMap）が
//! 解決時に参照する terminal-interception（terminal での横取り）分類。

// Action — カスタマイズ可能なユーザアクションすべて

// actions! マクロ（keymap-suite 0.1.2）が enum、ActionName 実装
// （from_name / name — 以前手書きだった from_str / as_str が提供していた
// 設定名のマッピング）、そして宣言順の Action::ALL（チートシートはこの順で
// 描画するので、宣言順がそのまま表示順になる）を生成する。デフォルトの
// チョードは意図的にここでは宣言していない: それらは default_keybinds.toml
// にあり、そのレイヤーごとのテーブルは、このアプリの9レイヤー・共有
// ナビゲーションのキーマップに、マクロの「アクションごとの "chord" @
// "layer" のリスト」よりもよく合う（スキーマと、ユーザが上書きする際に
// 読む根拠のコメントはそのファイルを参照）。
keymap_suite::actions! {
    pub enum Action {
        // グローバル
        Quit => "quit",
        ShowHelp => "show_help",
        CommandPalette => "command_palette",
        /// メニューバーにキーボードフォーカスを与える。矢印キーでタイトルを
        /// ブラウズでき、Down/Enter でリストが開く — GTK/Windows の慣習で
        /// あり、これがデフォルトのチョードが f10 である理由。Alt+文字の
        /// ニーモニックは使わない: alt+<文字> はすでにここで密に使われて
        /// いる（alt+h/l のフォーカス循環、alt+t のテーマピッカー、alt+w の
        /// レビュー解析）ので、ニーモニックのセットはそこから奪うことになる。
        FocusMenuBar => "focus_menu_bar",
        CycleFocusForward => "cycle_focus_forward",
        CycleFocusBackward => "cycle_focus_backward",
        /// どのパネルからでも、選択中の worktree を次/前のものに切り替える
        /// （worktree ストリップは選択に追従する）。フォーカス循環とは別物。
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
        /// フルスクリーンのコメント一覧モーダルを開く（ブランチ上の全コメントの
        /// 概観で、該当箇所へジャンプできる）。
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
        /// Viewer に表示中のファイルを外部エディタ（$VISUAL / $EDITOR）で開く:
        /// TUI を一時停止し、エディタを実行してから、復帰して再読み込みする。
        OpenInEditor => "open_in_editor",
        /// Viewer 内の markdown ファイルを raw ソースとレンダリング済みの
        /// プロースの間で切り替える。他のファイル（および diff モード）では
        /// 何もしない。両方のビューが異なるのは markdown だけだから。
        ToggleMarkdownRender => "toggle_markdown_render",

        // ターミナルパネル
        LeaveTerminal => "leave_terminal",
        ScrollbackUp => "scrollback_up",
        ScrollbackDown => "scrollback_down",
        ScrollbackTop => "scrollback_top",
        SnapToLive => "snap_to_live",
        OpenFileFromTerminal => "open_file_from_terminal",
        /// フォーカスされている terminal パネル（Claude Code か Shell）で、
        /// 次/前のセッションタブへ切り替える — タブをクリックするのと
        /// キーボード的に等価な操作。
        NextSession => "next_session",
        PrevSession => "prev_session",

        // アプリ
        UpdateAndRestart => "update_and_restart",

        // 検索
        SearchFullText => "search_full_text",

        // コードナビゲーション
        JumpBack => "jump_back",
        JumpForward => "jump_forward",
        /// viewer カーソル下のシンボルについて、型/シグネチャ、doc コメント、
        /// 参照数を示す hover ポップアップを表示する。
        ShowHoverInfo => "show_hover_info",
        ToggleInlineThread => "toggle_inline_thread",
        InlineReply => "inline_reply",

        // 差分ナビゲーション
        NextHunk => "next_hunk",
        PrevHunk => "prev_hunk",
        NextComment => "next_comment",
        PrevComment => "prev_comment",
        /// diff リスト内の次/前の変更ファイルへジャンプする（GitHub 風の
        /// 「次のファイル」 — ファイルをまたいだスクロールの軽量な代替）。
        NextChangedFile => "next_changed_file",
        PrevChangedFile => "prev_changed_file",

        // 差分コンテキストの展開
        ExpandContext => "expand_context",
        ExpandAllContext => "expand_all_context",

        // パネルレイアウト
        TogglePanelExpand => "toggle_panel_expand",
        TogglePanelOverlay => "toggle_panel_overlay",
        /// フォーカス中のパネルを左へ広げる（tmux の resize-pane -L）。
        ResizePaneLeft => "resize_pane_left",
        /// フォーカス中のパネルを右へ広げる（tmux の resize-pane -R）。
        ResizePaneRight => "resize_pane_right",
        /// フォーカス中のパネルを上へ広げる（tmux の resize-pane -U）。
        ResizePaneUp => "resize_pane_up",
        /// フォーカス中のパネルを下へ広げる（tmux の resize-pane -D）。
        ResizePaneDown => "resize_pane_down",

        // UI
        /// 実行時に UI のカラーテーマを切り替えるため、テーマピッカーの
        /// オーバーレイを開く。
        OpenThemePicker => "open_theme_picker",

        // PR レビュー
        /// revidere の 2 列レビュービュー (節一覧 + diff) を開く。成果物が
        /// 無ければ開かず、その場で作るかを聞く。
        ShowRevidere => "show_revidere",
        /// 2 列ビューの中で、次/前の節へ飛ぶ。左の選択と右の diff が一緒に動く。
        RevidereNextSection => "revidere_next_section",
        RevidererPrevSection => "revidere_prev_section",
        /// 総括の 1 列表示へ / 節 + diff の 2 列表示へ。
        ///
        /// 1 つのキーで交互に切り替えないのは、押した結果がいまどちらを
        /// 出しているかで変わるため。行き先が決まっているほうが、キーの
        /// 割り当てを説明するのも試すのも素直になる。
        RevidereShowOverview => "revidere_show_overview",
        RevidereShowSections => "revidere_show_sections",
        /// 上の 2 つと違って交互に切り替えるのは、行き先が 2 つしかなく、
        /// どちらを見ているかが画面の見出しに常に出ているため。
        RevidereToggleScope => "revidere_toggle_scope",
        /// ファイル（diff リストの行、または現在 Viewer の diff モードで
        /// 開いているファイル）の「viewed」マークを切り替える。
        ToggleViewed => "toggle_viewed",
        /// 'z' — コードブロックの折りたたみの2打鍵目（za/zc/zo/zR/zM）を待つ。
        /// 2打鍵目そのものは gd/gi/gr と同じくハンドラ側で直接読む: 折りたたみ
        /// だけが独自の再割り当て可能な語彙を持つ理由がない。
        FoldPrefix => "fold_prefix",
        /// PR番号/URL 入力のオーバーレイを開き、プルリクエスト用の worktree を
        /// 取得する（または既存のものを再利用する）。
        ReviewPullRequest => "review_pull_request",
        /// 選択中の worktree のレビューを作る。数分と費用がかかるので、
        /// 走り出す前に確認を挟む。
        AnalyzeRevidere => "analyze_revidere",
        /// 確認も、貯めた応答も飛ばして聞き直す (revidere の --no-cache)。
        /// プロンプトを直したときなど、diff が同じでも結果を作り直したい
        /// 場合の抜け道。
        ForceAnalyzeRevidere => "force_analyze_revidere",
        /// このブランチの未公開のレビューコメントを、それらが開かれた元の
        /// GitHub PR に公開する。パレット限定: 取り消せない外部アクションであり、
        /// 常に先に y/n の確認オーバーレイを通す。
        PublishReview => "publish_review",
    }
}

impl Action {
    /// チートシート向けの、人間が読めるアクションの1行説明。
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
            Action::FoldPrefix => "Fold block (za/zc/zo/zR/zM)",
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

    /// terminal パネル（PTY）がフォーカスされている間、このアクションが
    /// 横取りされるかどうか。false（デフォルト）は、そのチョードが内側の
    /// プログラム（shell / Claude Code）へ転送されることを意味し、Conductor
    /// はプログラムが必要とするキー（ctrl+r の reverse-search、ctrl+q/XON
    /// など）を奪うことがない。ここに列挙されている
    /// フォーカス/ナビゲーション/scrollback のアクションだけが奪い返される。
    /// これが terminal 横取りの単一の真実の源であり、
    /// KeyMap::resolve（super::KeyMap::resolve）と
    /// KeyMap::keys_for_action（super::KeyMap::keys_for_action）の両方が
    /// これに従うので、解決結果 == 実際の挙動 == 表示されるヘルプとなり、
    /// ディスパッチャ側に手作業で保守する許可リストは存在しない。
    pub(crate) fn fires_in_terminal(self) -> bool {
        matches!(
            self,
            // terminal 限定のアクション（terminal がフォーカスされている
            // ときにのみ意味を持つ）。
            Action::LeaveTerminal
                | Action::ScrollbackUp
                | Action::ScrollbackDown
                | Action::ScrollbackTop
                | Action::SnapToLive
                | Action::OpenFileFromTerminal
                // セッションタブの循環は、定義からして terminal パネルの
                // アクションである。
                | Action::NextSession
                | Action::PrevSession
                // PTY 上でも有用であり続けるグローバルなフォーカス/ナビゲーション。
                | Action::FocusWorktree
                | Action::FocusExplorer
                | Action::FocusExplorerDiffList
                | Action::FocusViewer
                | Action::FocusTerminalClaude
                | Action::FocusTerminalShell
                | Action::CommandPalette
                // メニューバーは terminal パネルからも到達可能でなければ
                // ならない — 時間の大半はそこで過ごされるのであり、他の
                // 何かにフォーカスしてからでないと開けないメニューは、
                // 使われなくなるメニューである。
                | Action::FocusMenuBar
                | Action::CycleFocusForward
                | Action::CycleFocusBackward
                | Action::NextWorktree
                | Action::PrevWorktree
                | Action::TogglePanelExpand
                | Action::TogglePanelOverlay
                // ペインのリサイズは terminal がフォーカスされているときに
                // 最も有用（そこに入力しながら Claude/Shell の分割や
                // terminal カラムをリサイズする）なので、これらも PTY 上で
                // 発火しなければならない。
                | Action::ResizePaneLeft
                | Action::ResizePaneRight
                | Action::ResizePaneUp
                | Action::ResizePaneDown
        )
    }
}
