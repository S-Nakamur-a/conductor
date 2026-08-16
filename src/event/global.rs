//! terminal 系以外のパネルで共有されるグローバルアクションのディスパッチ。

use crate::app::{App, Focus, StatusLevel};
use crate::keymap::Action;
use crate::overlay::ActiveOverlay;

/// terminal 系以外のパネルで共有されるグローバルアクションをディスパッチする。
/// アクションを処理したら true を返す。
pub(super) fn dispatch_global_action(app: &mut App, action: Action) -> bool {
    match action {
        Action::Quit => {
            app.quit();
            true
        }
        Action::ShowHelp => {
            app.overlays.help.context = app.focus;
            app.overlays.active = ActiveOverlay::Help;
            true
        }
        Action::CommandPalette => {
            app.overlays.active = ActiveOverlay::CommandPalette;
            app.overlays.command_palette.filter.clear();
            app.overlays.command_palette.selected = 0;
            true
        }
        Action::FocusMenuBar => {
            // リストを開かずにバーへフォーカスするだけ。開くのは Down/Enter の
            // 役目。オーバーレイが出ていないことをゲートにしている点は、キー
            // ディスパッチャがオーバーレイより先にメニューをチェックする際に
            // 依拠している不変条件と同じ。
            if app.overlays.active == ActiveOverlay::None {
                app.menu.focus_bar(0);
            }
            true
        }
        Action::OpenCommentList => {
            app.viewer_state.explorer.comment_list_selected = 0;
            app.viewer_state.explorer.comment_list_scroll = 0;
            app.overlays.active = ActiveOverlay::CommentList;
            true
        }
        Action::CycleFocusForward => {
            app.cycle_focus_forward();
            true
        }
        Action::CycleFocusBackward => {
            app.cycle_focus_backward();
            true
        }
        Action::NextWorktree => {
            app.select_next_worktree();
            true
        }
        Action::PrevWorktree => {
            app.select_prev_worktree();
            true
        }
        Action::FocusWorktree => {
            app.set_focus(Focus::Worktree);
            true
        }
        Action::FocusExplorer => {
            app.set_focus(Focus::Explorer);
            true
        }
        Action::FocusExplorerDiffList => {
            app.set_focus(Focus::Explorer);
            app.viewer_state.explorer.explorer_focus_on_diff_list = true;
            true
        }
        Action::FocusViewer => {
            app.set_focus(Focus::Viewer);
            true
        }
        Action::FocusTerminalClaude => {
            app.set_focus(Focus::TerminalClaude);
            true
        }
        Action::FocusTerminalShell => {
            app.set_focus(Focus::TerminalShell);
            true
        }
        Action::NewClaudeCode => {
            app.set_status("Starting Claude Code...".to_string(), StatusLevel::Info);
            if let Err(e) = app.spawn_claude_code() {
                app.set_status(
                    format!("Failed to start Claude Code: {e}"),
                    StatusLevel::Error,
                );
                log::warn!("failed to spawn Claude Code session: {e}");
            } else {
                app.status_message = None;
            }
            if app.focus != Focus::TerminalClaude {
                app.set_focus(Focus::TerminalClaude);
            }
            true
        }
        Action::NewShell => {
            app.set_status("Starting shell...".to_string(), StatusLevel::Info);
            if let Err(e) = app.spawn_shell() {
                app.set_status(format!("Failed to start shell: {e}"), StatusLevel::Error);
                log::warn!("failed to spawn shell session: {e}");
            } else {
                app.status_message = None;
            }
            if app.focus != Focus::TerminalShell {
                app.set_focus(Focus::TerminalShell);
            }
            true
        }
        Action::OpenRepo => {
            app.overlays.active = ActiveOverlay::OpenRepo;
            app.overlays
                .open_repo
                .buffer
                .set_text(&app.repo.path.display().to_string());
            true
        }
        Action::SwitchRepo => {
            if app.repo.known.len() > 1 {
                app.overlays.active = ActiveOverlay::RepoSelector;
                app.overlays.repo_selector.selected = app.repo.known_index;
            }
            true
        }
        Action::ReviewPullRequest => {
            app.overlays.active = ActiveOverlay::PrInput;
            app.overlays.pr_input.buffer.clear();
            app.overlays.pr_input.loading = false;
            app.overlays.pr_input.error = None;
            true
        }
        Action::UpdateAndRestart => {
            if app.update.info.is_some() {
                app.start_update_confirm();
            }
            true
        }
        Action::SearchFullText => {
            app.overlays.active = ActiveOverlay::GrepSearch;
            app.overlays.grep_search.query.clear();
            app.overlays.grep_search.result_tree = Default::default();
            app.overlays.grep_search.pending_matches.clear();
            app.overlays.grep_search.selected = 0;
            app.overlays.grep_search.scroll = 0;
            app.overlays.grep_search.running = false;
            app.overlays.grep_search.bg_op.clear();
            app.overlays.grep_search.bg_op_phase2.clear();
            app.overlays.grep_search.debounce_deadline = None;
            app.overlays.grep_search.phase1_active = false;
            app.overlays.grep_search.input_focused = true;
            true
        }
        Action::TogglePanelExpand => {
            if app.expanded_panel == Some(app.focus) {
                app.expanded_panel = None;
            } else {
                app.expanded_panel = Some(app.focus);
            }
            true
        }
        Action::TogglePanelOverlay => {
            app.panel_number_overlay.toggle();
            true
        }
        Action::ResizePaneLeft => {
            app.resize_focused_pane(crate::app::ResizeDir::Left);
            true
        }
        Action::ResizePaneRight => {
            app.resize_focused_pane(crate::app::ResizeDir::Right);
            true
        }
        Action::ResizePaneUp => {
            app.resize_focused_pane(crate::app::ResizeDir::Up);
            true
        }
        Action::ResizePaneDown => {
            app.resize_focused_pane(crate::app::ResizeDir::Down);
            true
        }
        Action::OpenThemePicker => {
            app.cmd_open_theme_picker();
            true
        }
        Action::AnalyzeRevidere => {
            app.cmd_confirm_analyze_revidere();
            true
        }
        Action::ForceAnalyzeRevidere => {
            app.cmd_analyze_revidere(true);
            true
        }
        Action::ShowRevidere => {
            app.cmd_show_revidere();
            true
        }
        Action::PublishReview => {
            app.cmd_publish_review();
            true
        }
        _ => false, // グローバルアクションではない — パネル固有のハンドラに任せる。
    }
}
