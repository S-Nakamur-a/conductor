//! コマンドパレットのディスパッチと、worktree やレビューコメントに紐づかない
//! 雑多なコマンドハンドラ群: テーマの切り替え、terminal/search の
//! 入口、リポジトリ/コメント一覧のナビゲーションショートカット。

use super::focus::Focus;
use super::panel_resize::ResizeDir;
use super::{App, StatusLevel, WorktreeInputMode};
use crate::overlay::ActiveOverlay;

impl App {
    pub fn execute_palette_command(&mut self, id: crate::command_palette::CommandId) {
        use crate::command_palette::CommandId;
        match id {
            // ナビゲーション
            CommandId::FocusWorktree => self.set_focus(Focus::Worktree),
            CommandId::FocusExplorer => self.set_focus(Focus::Explorer),
            CommandId::FocusViewer => self.set_focus(Focus::Viewer),
            CommandId::FocusTerminalClaude => self.set_focus(Focus::TerminalClaude),
            CommandId::FocusTerminalShell => self.set_focus(Focus::TerminalShell),
            CommandId::NextWorktree => self.select_next_worktree(),
            CommandId::PrevWorktree => self.select_prev_worktree(),
            CommandId::TogglePanelExpand => self.cmd_toggle_panel_expand(),
            CommandId::ResizePaneLeft => self.resize_focused_pane(ResizeDir::Left),
            CommandId::ResizePaneRight => self.resize_focused_pane(ResizeDir::Right),
            CommandId::ResizePaneUp => self.resize_focused_pane(ResizeDir::Up),
            CommandId::ResizePaneDown => self.resize_focused_pane(ResizeDir::Down),
            CommandId::CreateWorktree => self.cmd_create_worktree(),
            CommandId::DeleteWorktree => self.cmd_delete_worktree(),
            CommandId::SwitchBranch => self.cmd_switch_branch(),
            CommandId::GrabBranch => self.cmd_grab_branch(),
            CommandId::PruneWorktrees => self.cmd_prune_worktrees(),
            CommandId::MergeToMain => self.cmd_merge_to_main(),
            CommandId::RefreshWorktrees => {
                let _ = self.refresh_worktrees();
            }
            CommandId::ResetMainToOrigin => self.cmd_reset_main_to_origin(),
            CommandId::CherryPick => self.cmd_cherry_pick(),
            CommandId::PullWorktree => self.start_pull_worktree(),
            CommandId::NewClaudeCode => self.cmd_new_claude_code(),
            CommandId::NewShell => self.cmd_new_shell(),
            CommandId::ResumeClaudeSession => self.cmd_resume_claude_session(),
            CommandId::RefreshDiff => self.refresh_diff(),
            CommandId::SearchInFile => self.cmd_search_in_file(),
            CommandId::ToggleHelp => self.cmd_toggle_help(),
            CommandId::ToggleMarkdownRender => self.cmd_toggle_markdown_render(),
            CommandId::ShowReviewComments => self.cmd_show_review_comments(),
            CommandId::ShowReviewTemplates => {
                self.review_state.template_picker_active = true;
            }
            CommandId::SessionHistory => self.cmd_session_history(),
            CommandId::ReviewPullRequest => self.cmd_review_pull_request(),
            CommandId::AnalyzeRevidere => self.cmd_confirm_analyze_revidere(),
            CommandId::ForceAnalyzeRevidere => self.cmd_analyze_revidere(true),
            CommandId::PublishReview => self.cmd_publish_review(),
            CommandId::OpenRepo => self.cmd_open_repo(),
            CommandId::SwitchRepo => self.cmd_switch_repo(),
            CommandId::UngrabBranch => self.cmd_ungrab_branch(),
            CommandId::ShowDiffList => self.cmd_show_diff_list(),
            CommandId::ShowCommentList => self.cmd_show_comment_list(),
            CommandId::ShowRevidere => self.cmd_show_revidere(),
            CommandId::AddReviewComment => self.cmd_add_review_comment(),
            CommandId::ViewCommentDetail => self.cmd_view_comment_detail(),
            CommandId::DeleteComment => self.cmd_delete_comment(),
            CommandId::ToggleCommentResolve => self.cmd_toggle_comment_resolve(),
            CommandId::EditComment => self.cmd_edit_comment(),
            CommandId::ReplyToComment => self.cmd_reply_to_comment(),
            CommandId::SaveSessionHistory => self.save_current_session_history(),
            CommandId::OpenPullRequest => self.open_pr_in_browser(),
            CommandId::UpdateAndRestart => self.cmd_update_and_restart(),
            CommandId::CheckForUpdate => self.cmd_check_for_update(),
            CommandId::ToggleHighContrast => self.cmd_toggle_high_contrast(),
            CommandId::SearchFullText => self.cmd_search_full_text(),
            CommandId::Quit => self.should_quit = true,
            CommandId::SwitchTheme => self.cmd_open_theme_picker(),
        }
    }

    /// テーマ選択オーバーレイを開く。
    ///
    /// theme_name を復元先として保持しておくことで、(ライブプレビューで動いた
    /// 後でも)Esc でピッカーを開いた時点で有効だったテーマに戻せる。
    pub fn cmd_open_theme_picker(&mut self) {
        let themes: Vec<String> = crate::theme::Theme::all_names()
            .iter()
            .map(|s| s.to_string())
            .collect();
        let selected = themes
            .iter()
            .position(|t| t == &self.theme_sel.name)
            .unwrap_or(0);
        self.overlays.theme_picker = crate::overlay::ThemePickerOverlay {
            themes,
            selected,
            original: self.theme_sel.name.clone(),
        };
        self.overlays.active = ActiveOverlay::ThemePicker;
    }

    // コマンドパレットのハンドラメソッド

    /// ハイコントラストのテーマ変換をその場で切り替え、選択を永続化し、
    /// テーマ依存のキャッシュを再構築して変更を即座に反映させる。
    fn cmd_toggle_high_contrast(&mut self) {
        self.theme_sel.high_contrast = !self.theme_sel.high_contrast;
        self.config.ui.high_contrast = self.theme_sel.high_contrast;
        self.theme = super::build_theme(&self.theme_sel.name, self.theme_sel.high_contrast);

        // テーマの色を描画済みの span に焼き込んでいるキャッシュは再構築が必要。
        self.markdown_cache.clear();
        self.reflow.last_width = 0;
        self.reflow.cache.clear();
        self.dirty.mark_all();

        if let Err(e) = crate::config::persist_ui_high_contrast(self.theme_sel.high_contrast) {
            log::warn!("failed to persist high_contrast: {e}");
        }
        let state = if self.theme_sel.high_contrast {
            "on"
        } else {
            "off"
        };
        self.set_status_info(format!("High contrast {state}"));
    }

    fn cmd_search_full_text(&mut self) {
        self.overlays.active = ActiveOverlay::GrepSearch;
        self.overlays.grep_search.reset();
    }

    fn cmd_new_claude_code(&mut self) {
        if let Err(e) = self.spawn_claude_code() {
            self.set_status(
                format!("Failed to start Claude Code: {e}"),
                StatusLevel::Error,
            );
        }
        self.set_focus(Focus::TerminalClaude);
    }

    fn cmd_new_shell(&mut self) {
        if let Err(e) = self.spawn_shell() {
            self.set_status(format!("Failed to start shell: {e}"), StatusLevel::Error);
        }
        self.set_focus(Focus::TerminalShell);
    }

    fn cmd_resume_claude_session(&mut self) {
        self.overlays.active = ActiveOverlay::ResumeSession;
        self.load_resume_sessions();
    }

    fn cmd_search_in_file(&mut self) {
        self.viewer_state.search.search_active = true;
        self.viewer_state.search.search_query.clear();
        self.set_focus(Focus::Viewer);
    }

    fn cmd_toggle_help(&mut self) {
        self.overlays.help.context = self.focus;
        self.overlays.active = ActiveOverlay::Help;
    }

    fn cmd_show_review_comments(&mut self) {
        self.viewer_state.explorer.explorer_bottom_view =
            crate::viewer::ExplorerBottomView::Comments;
        self.viewer_state.explorer.explorer_focus_on_diff_list = true;
        self.set_focus(Focus::Explorer);
    }

    fn cmd_session_history(&mut self) {
        self.overlays.active = ActiveOverlay::History;
        self.load_session_history();
    }

    fn cmd_open_repo(&mut self) {
        self.overlays.active = ActiveOverlay::OpenRepo;
        self.overlays
            .open_repo
            .buffer
            .set_text(&self.repo.path.display().to_string());
    }

    fn cmd_review_pull_request(&mut self) {
        self.overlays.active = ActiveOverlay::PrInput;
        self.overlays.pr_input.buffer.clear();
        self.overlays.pr_input.loading = false;
        self.overlays.pr_input.error = None;
    }

    fn cmd_switch_repo(&mut self) {
        if self.repo.known.len() > 1 {
            self.overlays.active = ActiveOverlay::RepoSelector;
            self.overlays.repo_selector.selected = self.repo.known_index;
        }
    }

    fn cmd_ungrab_branch(&mut self) {
        if self.worktree_mgr.grabbed_branch.is_none() {
            self.set_status(
                "Not grabbing — nothing to ungrab.".to_string(),
                StatusLevel::Warning,
            );
        } else {
            self.worktree_mgr.input_mode = WorktreeInputMode::ConfirmingUngrab;
            self.set_status(
                "Ungrab? Main will return to main branch. (y/n)".to_string(),
                StatusLevel::Warning,
            );
        }
    }

    fn cmd_show_diff_list(&mut self) {
        self.viewer_state.explorer.explorer_bottom_view =
            crate::viewer::ExplorerBottomView::DiffList;
        self.viewer_state.explorer.explorer_focus_on_diff_list = true;
        self.set_focus(Focus::Explorer);
    }

    fn cmd_show_comment_list(&mut self) {
        self.viewer_state.explorer.explorer_bottom_view =
            crate::viewer::ExplorerBottomView::Comments;
        self.viewer_state.explorer.explorer_focus_on_diff_list = true;
        self.set_focus(Focus::Explorer);
    }
}
