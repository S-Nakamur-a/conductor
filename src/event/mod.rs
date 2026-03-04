//! Event handling — maps keyboard and mouse events to application actions.
//!
//! Focus-based dispatching: Tab / Shift+Tab cycle between panels.
//! Overlay handlers (worktree input, cherry-pick, etc.) take priority.
//! Terminal-focused panels forward keys to the active PTY session.

mod explorer;
mod global;
mod mouse;
mod overlay;
mod terminal;
mod viewer;
mod worktree;

use crossterm::event::{KeyCode, KeyEvent};

use crate::app::{App, Focus, UpdateState};
use crate::keymap::{Action, KeyContext};
use crate::review_state::ReviewInputMode;

use self::global::dispatch_global_action;
use self::overlay::*;
use self::terminal::{forward_key_to_pty, spawn_terminal_session};
use self::worktree::handle_worktree_key;
use self::explorer::handle_explorer_key;
use self::viewer::handle_viewer_key;

// Re-export public API.
pub use self::mouse::handle_mouse_event;

/// Process a single key event, updating application state as needed.
pub fn handle_key_event(app: &mut App, key: KeyEvent) {
    // ── 1. Overlay handlers — consume ALL keys when active ────────────

    if app.worktree_mgr.skip_reason.is_some() {
        if key.code == KeyCode::Esc {
            app.worktree_mgr.skip_reason = None;
        }
        return;
    }
    if app.update_state != UpdateState::Idle {
        handle_update_key(app, key);
        return;
    }
    if app.review_state.comment_detail_active {
        handle_comment_detail_key(app, key);
        return;
    }
    if app.review_state.input_mode != ReviewInputMode::Normal {
        handle_review_input_key(app, key);
        return;
    }
    if app.worktree_mgr.input_mode != crate::app::WorktreeInputMode::Normal {
        handle_worktree_input_key(app, key);
        return;
    }
    if app.switch_branch.active {
        handle_switch_branch_key(app, key);
        return;
    }
    if app.grab.active {
        handle_grab_key(app, key);
        return;
    }
    if app.prune.active {
        handle_prune_key(app, key);
        return;
    }
    if app.cherry_pick.active {
        handle_cherry_pick_key(app, key);
        return;
    }
    if app.viewer_state.filename_search_active {
        handle_filename_search_key(app, key);
        return;
    }
    if app.viewer_state.search_active {
        handle_viewer_search_key(app, key);
        return;
    }
    if app.review_state.search_active {
        handle_review_search_key(app, key);
        return;
    }
    if app.review_state.template_picker_active {
        handle_review_template_key(app, key);
        return;
    }
    if app.history.active {
        handle_history_key(app, key);
        return;
    }
    if app.resume_session.active {
        handle_resume_session_key(app, key);
        return;
    }
    if app.repo_selector.active {
        handle_repo_selector_key(app, key);
        return;
    }
    if app.open_repo.active {
        handle_open_repo_key(app, key);
        return;
    }
    if app.grep_search.active {
        handle_grep_search_key(app, key);
        return;
    }
    if app.help.active {
        handle_help_key(app, key);
        return;
    }
    if app.command_palette.active {
        handle_command_palette_key(app, key);
        return;
    }

    // ── 1b. Terminal focus — intercept configurable keys, forward rest to PTY ─

    if app.focus == Focus::TerminalClaude || app.focus == Focus::TerminalShell {
        // Check terminal-specific and global bindings first.
        if let Some(action) = app.keymap.resolve(&key, KeyContext::Terminal) {
            match action {
                Action::LeaveTerminal => { app.set_focus(Focus::Explorer); return; }
                Action::FocusWorktree => { app.set_focus(Focus::Worktree); return; }
                Action::FocusExplorer => { app.set_focus(Focus::Explorer); return; }
                Action::FocusViewer => { app.set_focus(Focus::Viewer); return; }
                Action::FocusTerminalClaude => { app.set_focus(Focus::TerminalClaude); return; }
                Action::FocusTerminalShell => { app.set_focus(Focus::TerminalShell); return; }
                Action::CommandPalette => {
                    app.command_palette.active = true;
                    app.command_palette.filter.clear();
                    app.command_palette.selected = 0;
                    return;
                }
                Action::ScrollbackUp => {
                    let page = match app.focus {
                        Focus::TerminalClaude => app.terminal.size_claude.0 as usize / 2,
                        Focus::TerminalShell => app.terminal.size_shell.0 as usize / 2,
                        _ => unreachable!(),
                    };
                    let scroll = match app.focus {
                        Focus::TerminalClaude => &mut app.terminal.scroll_claude,
                        Focus::TerminalShell => &mut app.terminal.scroll_shell,
                        _ => unreachable!(),
                    };
                    *scroll = scroll.saturating_add(page.max(1));
                    return;
                }
                Action::ScrollbackDown => {
                    let page = match app.focus {
                        Focus::TerminalClaude => app.terminal.size_claude.0 as usize / 2,
                        Focus::TerminalShell => app.terminal.size_shell.0 as usize / 2,
                        _ => unreachable!(),
                    };
                    let scroll = match app.focus {
                        Focus::TerminalClaude => &mut app.terminal.scroll_claude,
                        Focus::TerminalShell => &mut app.terminal.scroll_shell,
                        _ => unreachable!(),
                    };
                    *scroll = scroll.saturating_sub(page.max(1));
                    return;
                }
                Action::ScrollbackTop => {
                    match app.focus {
                        Focus::TerminalClaude => app.terminal.scroll_claude = 1000,
                        Focus::TerminalShell => app.terminal.scroll_shell = 1000,
                        _ => unreachable!(),
                    }
                    return;
                }
                Action::SnapToLive => {
                    match app.focus {
                        Focus::TerminalClaude => app.terminal.scroll_claude = 0,
                        Focus::TerminalShell => app.terminal.scroll_shell = 0,
                        _ => unreachable!(),
                    }
                    return;
                }
                Action::TogglePanelExpand => {
                    if app.expanded_panel == Some(app.focus) {
                        app.expanded_panel = None;
                    } else {
                        app.expanded_panel = Some(app.focus);
                    }
                    return;
                }
                _ => {} // Other global actions not intercepted in terminal
            }
        }

        // Forward all remaining keys to the active PTY session.
        let session_idx = match app.focus {
            Focus::TerminalClaude => app.terminal.active_claude_session,
            Focus::TerminalShell => app.terminal.active_shell_session,
            _ => unreachable!(),
        };
        if let Some(idx) = session_idx {
            forward_key_to_pty(app, idx, key);
        } else if key.code == KeyCode::Enter {
            spawn_terminal_session(app);
        }
        return;
    }

    // ── 2. Non-terminal panels — resolve via keymap ──────────────────

    let context = match app.focus {
        Focus::Worktree => KeyContext::Worktree,
        Focus::Explorer => KeyContext::Explorer,
        Focus::Viewer => KeyContext::Viewer,
        Focus::TerminalClaude | Focus::TerminalShell => unreachable!(),
    };

    if let Some(action) = app.keymap.resolve(&key, context) {
        if dispatch_global_action(app, action) {
            return;
        }
    }

    // ── 3. Focus-specific keybindings ────────────────────────────────

    match app.focus {
        Focus::Worktree => handle_worktree_key(app, key),
        Focus::Explorer => handle_explorer_key(app, key),
        Focus::Viewer => handle_viewer_key(app, key),
        Focus::TerminalClaude | Focus::TerminalShell => unreachable!(),
    }
}

// ── Paste event handling ────────────────────────────────────────────────

/// Handle a bracketed paste event. When the terminal panel is focused,
/// forward the entire pasted text to the PTY in one write, wrapped with
/// bracketed-paste escape sequences so the shell/application treats it as
/// a single paste rather than individual keystrokes.
pub fn handle_paste_event(app: &mut App, data: String) {
    if app.focus != Focus::TerminalClaude && app.focus != Focus::TerminalShell {
        // Dispatch paste data to the active overlay input buffer.
        use crate::app::WorktreeInputMode;

        let single_line: String = data.chars().filter(|c| *c != '\n' && *c != '\r').collect();

        if app.review_state.input_mode != ReviewInputMode::Normal {
            // Review input is multiline.
            app.review_state.input_buffer.insert_str(&data);
        } else if app.worktree_mgr.input_mode == WorktreeInputMode::SmartDescription {
            // Smart description is multiline.
            app.worktree_mgr.smart_description_buffer.insert_str(&data);
        } else if app.worktree_mgr.input_mode == WorktreeInputMode::CreatingWorktree
            || app.worktree_mgr.input_mode == WorktreeInputMode::CreatingWorktreeBase
        {
            app.worktree_mgr.input_buffer.insert_str(&single_line);
        } else if app.viewer_state.search_active {
            app.viewer_state.search_query.insert_str(&single_line);
        } else if app.viewer_state.filename_search_active {
            app.viewer_state.filename_search_query.insert_str(&single_line);
        } else if app.review_state.search_active {
            app.review_state.search_query.insert_str(&single_line);
            app.review_state.apply_filter();
        } else if app.switch_branch.active {
            app.switch_branch.filter.insert_str(&single_line);
        } else if app.command_palette.active {
            app.command_palette.filter.insert_str(&single_line);
        } else if app.open_repo.active {
            app.open_repo.buffer.insert_str(&single_line);
        } else if app.history.active {
            app.history.search_query.insert_str(&single_line);
        } else if app.resume_session.active {
            app.resume_session.filter.insert_str(&single_line);
        }
        return;
    }

    let session_idx = match app.focus {
        Focus::TerminalClaude => app.terminal.active_claude_session,
        Focus::TerminalShell => app.terminal.active_shell_session,
        _ => None,
    };

    if let Some(idx) = session_idx {
        // Wrap the paste data with bracketed-paste escape sequences so
        // that the child process (shell, editor, claude-code) knows this
        // is pasted text and will not execute each line individually.
        let mut buf = Vec::with_capacity(data.len() + 12);
        buf.extend_from_slice(b"\x1b[200~");
        buf.extend_from_slice(data.as_bytes());
        buf.extend_from_slice(b"\x1b[201~");

        if let Err(e) = app.terminal.pty_manager.write_to_session(idx, &buf) {
            log::warn!("failed to write paste data to PTY session: {e}");
        } else {
            match app.focus {
                Focus::TerminalClaude => app.terminal.scroll_claude = 0,
                Focus::TerminalShell => app.terminal.scroll_shell = 0,
                _ => {}
            }
            app.clear_cc_waiting_signal(idx);
        }
    }
}

// ── Update overlay ──────────────────────────────────────────────────────

fn handle_update_key(app: &mut App, key: KeyEvent) {
    match app.update_state {
        UpdateState::Confirming => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                app.start_update_download();
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                app.update_state = UpdateState::Idle;
            }
            _ => {}
        },
        UpdateState::InProgress => {
            if key.code == KeyCode::Esc {
                app.update_op.clear();
                app.update_state = UpdateState::Idle;
            }
        }
        UpdateState::Failed => {
            // Any key dismisses the error.
            app.update_state = UpdateState::Idle;
        }
        UpdateState::Restarting | UpdateState::Idle => {}
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// Paste clipboard contents into the `TextInput` returned by `get_buffer`.
///
/// If `multiline` is false, newlines are stripped from the pasted text.
fn clipboard_paste<F>(app: &mut App, get_buffer: F, multiline: bool)
where
    F: FnOnce(&mut App) -> &mut crate::text_input::TextInput,
{
    use copypasta::ClipboardProvider;
    let text = app
        .clipboard
        .as_mut()
        .and_then(|ctx| ctx.get_contents().ok());
    if let Some(text) = text {
        let buf = get_buffer(app);
        if multiline {
            buf.insert_str(&text);
        } else {
            let cleaned: String = text.chars().filter(|c| *c != '\n' && *c != '\r').collect();
            buf.insert_str(&cleaned);
        }
    }
}

/// Adjust `tree_scroll` so that `tree_selected` stays visible.
fn adjust_tree_scroll(app: &mut App) {
    let visible = app.viewer_state.visible_indices();
    let cur_vis = visible
        .iter()
        .position(|&i| i == app.viewer_state.tree_selected)
        .unwrap_or(0);

    let page_size = app.viewer_state.explorer_tree_height.max(1);

    if cur_vis < app.viewer_state.tree_scroll {
        app.viewer_state.tree_scroll = cur_vis;
    } else if cur_vis >= app.viewer_state.tree_scroll + page_size {
        app.viewer_state.tree_scroll = cur_vis.saturating_sub(page_size - 1);
    }
}

/// Adjust `diff_list_scroll` so that `diff_list_selected` stays visible.
fn adjust_diff_list_scroll(app: &mut App) {
    let selected = app.viewer_state.diff_list_selected;
    let page_size = app.viewer_state.explorer_diff_list_height.max(1);

    if selected < app.viewer_state.diff_list_scroll {
        app.viewer_state.diff_list_scroll = selected;
    } else if selected >= app.viewer_state.diff_list_scroll + page_size {
        app.viewer_state.diff_list_scroll = selected.saturating_sub(page_size - 1);
    }
}
