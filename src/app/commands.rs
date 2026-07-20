//! Command-palette dispatch and miscellaneous command handlers not tied to
//! worktrees or review comments: theme/rich-mode toggles, terminal/search
//! entry points, and repo/comment-list navigation shortcuts.

use super::focus::Focus;
use super::panel_resize::ResizeDir;
use super::{App, StatusLevel, WorktreeInputMode};
use crate::overlay::ActiveOverlay;

impl App {
    pub fn execute_palette_command(&mut self, id: crate::command_palette::CommandId) {
        use crate::command_palette::CommandId;
        match id {
            // Navigation
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
            CommandId::ShowReviewComments => self.cmd_show_review_comments(),
            CommandId::ShowReviewTemplates => {
                self.review_state.template_picker_active = true;
            }
            CommandId::SessionHistory => self.cmd_session_history(),
            CommandId::ReviewPullRequest => self.cmd_review_pull_request(),
            CommandId::GenerateWalkthrough => self.cmd_generate_walkthrough(false),
            CommandId::ForceGenerateWalkthrough => self.cmd_generate_walkthrough(true),
            CommandId::PublishReview => self.cmd_publish_review(),
            CommandId::OpenRepo => self.cmd_open_repo(),
            CommandId::SwitchRepo => self.cmd_switch_repo(),
            CommandId::UngrabBranch => self.cmd_ungrab_branch(),
            CommandId::ShowDiffList => self.cmd_show_diff_list(),
            CommandId::ShowCommentList => self.cmd_show_comment_list(),
            CommandId::ShowWalkthrough => self.cmd_show_walkthrough(),
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
            CommandId::TogglePartyMode => self.cmd_toggle_party_mode(),
            CommandId::ToggleRichMode => self.cmd_toggle_rich_mode(),
            CommandId::Quit => self.should_quit = true,
            CommandId::SwitchTheme => self.cmd_open_theme_picker(),
        }
    }

    /// Open the theme picker overlay.
    ///
    /// Captures `theme_name` as the revert target so Esc can restore the theme
    /// that was active when the picker opened (even after live-preview moves).
    pub fn cmd_open_theme_picker(&mut self) {
        let themes: Vec<String> = crate::theme::Theme::all_names()
            .iter()
            .map(|s| s.to_string())
            .collect();
        let selected = themes
            .iter()
            .position(|t| t == &self.theme_name)
            .unwrap_or(0);
        self.overlays.theme_picker = crate::overlay::ThemePickerOverlay {
            themes,
            selected,
            original: self.theme_name.clone(),
        };
        self.overlays.active = ActiveOverlay::ThemePicker;
    }

    // ── Command palette handler methods ──────────────────────────────

    /// Toggle the hidden party theme mode (rainbow borders, flashy syntax,
    /// confetti). A flash message confirms the new state; the whole UI is
    /// re-rendered so the effect appears/disappears immediately.
    fn cmd_toggle_party_mode(&mut self) {
        self.party_mode = !self.party_mode;
        if self.party_mode {
            self.set_status("🎉 Party mode ON! 🎉".to_string(), StatusLevel::Success);
        } else {
            self.set_status_info("Party mode off.".to_string());
        }
        self.dirty.mark_all();
    }

    /// Toggle rich mode between off and the tier detected at startup. On
    /// terminals where detection found nothing, toggling on falls back to
    /// Tier A (same behaviour as `[rich] mode = "force"`).
    fn cmd_toggle_rich_mode(&mut self) {
        use crate::term_caps::RichTier;
        if self.rich_tier.is_rich() {
            self.rich_tier = RichTier::Off;
            self.set_status_info("Rich mode off.".to_string());
        } else {
            self.rich_tier = if self.rich_tier_available.is_rich() {
                self.rich_tier_available
            } else {
                RichTier::TierA
            };
            self.set_status("✨ Rich mode ON".to_string(), StatusLevel::Success);
        }
        self.dirty.mark_all();
    }

    /// Toggle the high-contrast theme transform live, persist the choice, and
    /// rebuild the theme-dependent caches so the change is visible immediately.
    fn cmd_toggle_high_contrast(&mut self) {
        self.high_contrast = !self.high_contrast;
        self.config.ui.high_contrast = self.high_contrast;
        self.theme = super::build_theme(&self.theme_name, self.high_contrast);

        // Caches that bake theme colours into rendered spans must be rebuilt.
        self.markdown_cache.clear();
        self.reflow.last_width = 0;
        self.reflow.cache.clear();
        self.dirty.mark_all();

        if let Err(e) = crate::config::persist_ui_high_contrast(self.high_contrast) {
            log::warn!("failed to persist high_contrast: {e}");
        }
        let state = if self.high_contrast { "on" } else { "off" };
        self.set_status_info(format!("High contrast {state}"));
    }

    fn cmd_search_full_text(&mut self) {
        self.overlays.active = ActiveOverlay::GrepSearch;
        self.overlays.grep_search.query.clear();
        self.overlays.grep_search.result_tree = Default::default();
        self.overlays.grep_search.pending_matches.clear();
        self.overlays.grep_search.selected = 0;
        self.overlays.grep_search.scroll = 0;
        self.overlays.grep_search.running = false;
        self.overlays.grep_search.bg_op.clear();
        self.overlays.grep_search.bg_op_phase2.clear();
        self.overlays.grep_search.debounce_deadline = None;
        self.overlays.grep_search.phase1_active = false;
        self.overlays.grep_search.input_focused = true;
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
            .set_text(&self.repo_path.display().to_string());
    }

    fn cmd_review_pull_request(&mut self) {
        self.overlays.active = ActiveOverlay::PrInput;
        self.overlays.pr_input.buffer.clear();
        self.overlays.pr_input.loading = false;
        self.overlays.pr_input.error = None;
    }

    fn cmd_switch_repo(&mut self) {
        if self.repo_list.len() > 1 {
            self.overlays.active = ActiveOverlay::RepoSelector;
            self.overlays.repo_selector.selected = self.repo_list_index;
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
        self.viewer_state.explorer.explorer_bottom_view = crate::viewer::ExplorerBottomView::DiffList;
        self.viewer_state.explorer.explorer_focus_on_diff_list = true;
        self.set_focus(Focus::Explorer);
    }

    fn cmd_show_comment_list(&mut self) {
        self.viewer_state.explorer.explorer_bottom_view =
            crate::viewer::ExplorerBottomView::Comments;
        self.viewer_state.explorer.explorer_focus_on_diff_list = true;
        self.set_focus(Focus::Explorer);
    }

    /// Palette/keybinding entry point: switch the Explorer's bottom pane to
    /// the AI walkthrough view and focus the Explorer, mirroring
    /// `cmd_show_diff_list`/`cmd_show_comment_list`. `cmd_generate_walkthrough`
    /// uses a display-only variant instead (see its doc comment) so kicking
    /// off a generation never steals focus from an active terminal input.
    fn cmd_show_walkthrough(&mut self) {
        self.viewer_state.explorer.explorer_bottom_view =
            crate::viewer::ExplorerBottomView::Walkthrough;
        self.viewer_state.explorer.explorer_focus_on_diff_list = true;
        self.set_focus(Focus::Explorer);
    }
}
