//! Global action dispatch shared across non-terminal panels.

use crate::app::{App, Focus, StatusLevel};
use crate::keymap::Action;
use crate::overlay::ActiveOverlay;

/// Dispatch global actions that are shared across non-terminal panels.
/// Returns `true` if the action was handled.
pub(super) fn dispatch_global_action(app: &mut App, action: Action) -> bool {
    match action {
        Action::Quit => { app.quit(); true }
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
        Action::CycleFocusForward => { app.cycle_focus_forward(); true }
        Action::CycleFocusBackward => { app.cycle_focus_backward(); true }
        Action::FocusWorktree => { app.set_focus(Focus::Worktree); true }
        Action::FocusExplorer => { app.set_focus(Focus::Explorer); true }
        Action::FocusViewer => { app.set_focus(Focus::Viewer); true }
        Action::FocusTerminalClaude => { app.set_focus(Focus::TerminalClaude); true }
        Action::FocusTerminalShell => { app.set_focus(Focus::TerminalShell); true }
        Action::NewClaudeCode => {
            app.set_status("Starting Claude Code...".to_string(), StatusLevel::Info);
            if let Err(e) = app.spawn_claude_code() {
                app.set_status(format!("Failed to start Claude Code: {e}"), StatusLevel::Error);
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
            app.overlays.open_repo.buffer.set_text(&app.repo_path.display().to_string());
            true
        }
        Action::SwitchRepo => {
            if app.repo_list.len() > 1 {
                app.overlays.active = ActiveOverlay::RepoSelector;
                app.overlays.repo_selector.selected = app.repo_list_index;
            }
            true
        }
        Action::UpdateAndRestart => {
            if app.update_info.is_some() {
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
        _ => false, // Not a global action — let panel-specific handler try.
    }
}
