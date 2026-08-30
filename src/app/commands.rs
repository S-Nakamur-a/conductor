//! コマンドパレットのディスパッチと、worktree やレビューコメントに紐づかない
//! 雑多なコマンドハンドラ群: テーマの切り替え、terminal/search の
//! 入口、リポジトリ/コメント一覧のナビゲーションショートカット。

use super::panel_resize::ResizeDir;
use super::{App, StatusLevel, WorktreeInputMode};
use crate::overlay::ActiveOverlay;
use crate::types::Focus;

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
            CommandId::FoldOneLevel => self.cmd_fold_one_level(),
            CommandId::UnfoldOneLevel => self.cmd_unfold_one_level(),
            CommandId::FoldAll => self.cmd_fold_all(),
            CommandId::UnfoldAll => self.cmd_unfold_all(),
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
            CommandId::RebuildCodeIndex => self.cmd_rebuild_code_index(),
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
    /// id が現在のアプリ状態に対して実行可能かどうか — メニューのグレーアウト判定。
    ///
    /// デフォルトは実行可能とする。ここにケースを書くのは、コマンド自身がすでに
    /// 具体的な箇所で実行を拒否している場合だけで、各ケースにはその箇所を明記する。
    /// この非対称性は意図的である。誤って false にすると、全操作を一覧するはずの
    /// この UI から動くはずの操作が黙って消える。一方 true を誤っても今日の挙動
    /// 以上の代償はない — コマンドはそのまま実行され、パレットから実行したときと
    /// 同じようにステータスバーで結果を報告する。
    ///
    /// したがってケースを追加する際のルールは、対応する既存のチェック箇所を指し示す
    /// ことである。コマンド名から前提条件を推測して作ってはいけない。
    ///
    /// 本当の前提条件が I/O(SQLite の読み取りや git の呼び出し)を要する場合は、
    /// そのうち安価な部分だけをここで再現する。開いているドロップダウンの表示中の
    /// 行すべてに対して毎フレーム呼ばれるため、レビュー DB に問い合わせるような
    /// 判定はレンダーループの中に DB ラウンドトリップを持ち込むことになる。
    pub fn command_enabled(&self, id: crate::command_palette::CommandId) -> bool {
        let selected_worktree = self.worktrees.selected();

        match id {
            // App
            // Action::UpdateAndRestart はリリースが見つかっている場合以外は
            // 何もしない(event/global.rs、if self.update.info.is_some())。
            crate::command_palette::CommandId::UpdateAndRestart => self.update.info.is_some(),

            // Repository
            // リポジトリ選択は切替先が複数あるときのみ開く
            // (event/global.rs、if self.repo.known.len() > 1)。
            crate::command_palette::CommandId::SwitchRepo => self.repo.known.len() > 1,

            // Worktree
            // ストリップの削除ボタンは main worktree と削除処理中の worktree を
            // 拒否する(event/mouse/bars.rs)。
            crate::command_palette::CommandId::DeleteWorktree => selected_worktree
                .is_some_and(|w| !w.is_main && !self.is_worktree_pending_delete(&w.path)),

            // "Cannot merge main into itself."(app/worktree_commands.rs)。
            crate::command_palette::CommandId::MergeToMain => {
                selected_worktree.is_some_and(|w| !w.is_main)
            }

            // "Already grabbing a branch. Ungrab first (G)."
            // (app/worktree_commands.rs)。後続の「grab 可能な非 main worktree が
            // ない」というチェックはここでは再現しない。オーバーレイ状態を変更する
            // load_grab_branches() が必要になるため。
            crate::command_palette::CommandId::GrabBranch => {
                self.worktree_mgr.grabbed_branch.is_none()
            }

            // "Not grabbing — nothing to ungrab."(app/commands.rs)。
            crate::command_palette::CommandId::UngrabBranch => {
                self.worktree_mgr.grabbed_branch.is_some()
            }

            // "No worktree selected."(app/worktree_pr.rs)。ブランチに実際に
            // PR があるかどうかは git 呼び出しが必要なので、そこはコマンド側に
            // 任せる。
            crate::command_palette::CommandId::OpenPullRequest => selected_worktree.is_some(),

            // Viewer
            // "Raw/Rendered applies to a markdown file in the Viewer"
            // (app/view_state.rs) — コマンドが参照するのと同じヘルパー。
            crate::command_palette::CommandId::ToggleMarkdownRender => {
                self.viewer.markdown_toggle_available()
            }

            // 畳める範囲を持つファイルを Viewer が表示しているときだけ意味を持つ。
            crate::command_palette::CommandId::FoldOneLevel
            | crate::command_palette::CommandId::UnfoldOneLevel
            | crate::command_palette::CommandId::FoldAll
            | crate::command_palette::CommandId::UnfoldAll => self.viewer.folds_available(),

            // Review
            // レビュー DB と worktree が必要(app/review_publish.rs)。ブランチに
            // 紐づく PR があるかは get_pr_review_meta クエリが必要なので、
            // そこはコマンド側に任せる。
            crate::command_palette::CommandId::PublishReview => {
                self.review_store.is_some() && selected_worktree.is_some()
            }

            _ => true,
        }
    }

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
        self.request_redraw();

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
        self.viewer.search.search_active = true;
        self.viewer.search.search_query.clear();
        self.set_focus(Focus::Viewer);
    }

    fn cmd_toggle_help(&mut self) {
        self.overlays.help.context = self.focus;
        self.overlays.active = ActiveOverlay::Help;
    }

    fn cmd_show_review_comments(&mut self) {
        self.explorer.bottom_view = crate::explorer::ExplorerBottomView::Comments;
        self.explorer.focus_on_diff_list = true;
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
        self.explorer.bottom_view = crate::explorer::ExplorerBottomView::DiffList;
        self.explorer.focus_on_diff_list = true;
        self.set_focus(Focus::Explorer);
    }

    fn cmd_show_comment_list(&mut self) {
        self.explorer.bottom_view = crate::explorer::ExplorerBottomView::Comments;
        self.explorer.focus_on_diff_list = true;
        self.set_focus(Focus::Explorer);
    }
}
