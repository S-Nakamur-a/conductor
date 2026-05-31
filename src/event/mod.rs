//! Event handling — maps keyboard and mouse events to application actions.
//!
//! Focus-based dispatching: Tab / Shift+Tab cycle between non-terminal panels;
//! Alt+h / Alt+l cycle between all panels including terminals.
//! Overlay handlers (worktree input, cherry-pick, etc.) take priority.
//! Terminal-focused panels forward keys to the active PTY session.

mod explorer;
mod global;
mod mouse;
mod overlay;
mod terminal;
mod viewer;
mod worktree;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, Focus, UpdateState, WorktreeInputMode};
use crate::keymap::{Action, KeyContext};
use crate::overlay::ActiveOverlay;
use crate::review_state::ReviewInputMode;

use self::explorer::handle_explorer_key;
use self::explorer::handle_explorer_comment_list_key;
use self::global::dispatch_global_action;
use self::overlay::*;
use self::terminal::{forward_key_to_pty, spawn_terminal_session};
use self::viewer::handle_viewer_key;
use self::worktree::handle_worktree_key;

// ── Effective overlay ───────────────────────────────────────────────────

/// Unified overlay/modal state for dispatch. Collapses the multiple
/// boolean/enum checks into a single discriminant.
enum EffectiveOverlay {
    /// Skip-reason modal (worktree creation failure detail).
    SkipReason,
    /// Update confirmation/progress/failure dialog.
    UpdateState,
    /// Comment detail popup.
    CommentDetail,
    /// Review text input (add/edit/reply).
    ReviewInput,
    /// Worktree text input (create/confirm/smart).
    WorktreeInput,
    /// An `ActiveOverlay` variant (switch-branch, cherry-pick, etc.).
    Active(ActiveOverlay),
    /// Filename search sub-modal.
    FilenameSearch,
    /// Viewer in-file search sub-modal.
    ViewerSearch,
    /// Review comment search sub-modal.
    ReviewSearch,
    /// Review template picker sub-modal.
    ReviewTemplate,
    /// No overlay — dispatch to focused panel.
    None,
}

/// Determine the single effective overlay/modal that should consume input.
fn effective_overlay(app: &App) -> EffectiveOverlay {
    if app.worktree_mgr.skip_reason.is_some() {
        return EffectiveOverlay::SkipReason;
    }
    if app.update_state != UpdateState::Idle {
        return EffectiveOverlay::UpdateState;
    }
    if app.review_state.comment_detail_active {
        return EffectiveOverlay::CommentDetail;
    }
    if app.review_state.input_mode != ReviewInputMode::Normal {
        return EffectiveOverlay::ReviewInput;
    }
    if app.worktree_mgr.input_mode != WorktreeInputMode::Normal {
        return EffectiveOverlay::WorktreeInput;
    }
    match app.overlays.active {
        ActiveOverlay::None => {}
        other => return EffectiveOverlay::Active(other),
    }
    if app.viewer_state.filename_search.filename_search_active {
        return EffectiveOverlay::FilenameSearch;
    }
    if app.viewer_state.search.search_active {
        return EffectiveOverlay::ViewerSearch;
    }
    if app.review_state.search_active {
        return EffectiveOverlay::ReviewSearch;
    }
    if app.review_state.template_picker_active {
        return EffectiveOverlay::ReviewTemplate;
    }
    EffectiveOverlay::None
}

/// True when a text-entry field is currently focused and expects printable
/// characters to be inserted as literal text — every input target enumerated in
/// [`handle_paste_event`]. Kept in lockstep with that function: a destination
/// added there must be added here too. Notably this is `false` for the
/// `WorktreeInputMode::Confirming*` y/n sub-modes, which are NOT text entry.
fn is_text_input_active(app: &App) -> bool {
    if app.viewer_state.explorer.inline_reply_line.is_some()
        || app.review_state.input_mode != ReviewInputMode::Normal
        || app.review_state.search_active
        || app.viewer_state.search.search_active
        || app.viewer_state.filename_search.filename_search_active
    {
        return true;
    }
    if matches!(
        app.worktree_mgr.input_mode,
        WorktreeInputMode::CreatingWorktree
            | WorktreeInputMode::CreatingWorktreeBase
            | WorktreeInputMode::SmartDescription
    ) {
        return true;
    }
    matches!(
        app.overlays.active,
        ActiveOverlay::GrepSearch
            | ActiveOverlay::SwitchBranch
            | ActiveOverlay::CommandPalette
            | ActiveOverlay::OpenRepo
            | ActiveOverlay::History
            | ActiveOverlay::ResumeSession
    )
}

/// True when `key` is a printable character carrying no command modifier
/// (Ctrl/Alt/Super). A lone Shift still counts as "bare" — `Shift+a` is just
/// `A`. Such a key is indistinguishable from typed text, so it must never be
/// hijacked as a global accelerator while a text field is focused. This is what
/// stops the macOS Option-glyph focus fallbacks (`¡ ™ £ ¢ ∞ §` …) from stealing
/// focus mid-IME-input.
fn is_bare_printable(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char(_))
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
}

// Re-export public API.
pub use self::mouse::handle_mouse_event;

/// Process a single key event, updating application state as needed.
pub fn handle_key_event(app: &mut App, key: KeyEvent) {
    // ── 0. Global focus-switching — available even over overlays, EXCEPT a
    // bare printable key while a text field is focused, which must reach the
    // text buffer. The global layer binds macOS Option-glyph fallbacks (bare
    // '¡ ™ £ ¢ ∞ § ÷ ˙ ¬') to focus actions; those glyphs are indistinguishable
    // from typed text, so during IME / multi-byte input they would otherwise
    // steal focus (e.g. '∞' jumping to the Claude panel). Modifier-carrying
    // chords (alt+1, ctrl+w, super+1, alt+h …) still switch focus over a modal.
    if !(is_text_input_active(app) && is_bare_printable(&key))
        && let Some(action) = app.keymap.resolve(&key, KeyContext::Global)
    {
        match action {
            Action::FocusWorktree
            | Action::FocusExplorer
            | Action::FocusExplorerDiffList
            | Action::FocusViewer
            | Action::FocusTerminalClaude
            | Action::FocusTerminalShell => {
                dismiss_overlays(app);
                dispatch_global_action(app, action);
                return;
            }
            _ => {}
        }
    }

    // ── 1. Overlay / modal dispatch — consume ALL keys when active ────

    match effective_overlay(app) {
        EffectiveOverlay::SkipReason => {
            if key.code == KeyCode::Esc {
                app.worktree_mgr.skip_reason = None;
            }
            return;
        }
        EffectiveOverlay::UpdateState => {
            handle_update_key(app, key);
            return;
        }
        EffectiveOverlay::CommentDetail => {
            handle_comment_detail_key(app, key);
            return;
        }
        EffectiveOverlay::ReviewInput => {
            handle_review_input_key(app, key);
            return;
        }
        EffectiveOverlay::WorktreeInput => {
            handle_worktree_input_key(app, key);
            return;
        }
        EffectiveOverlay::Active(overlay) => {
            match overlay {
                ActiveOverlay::SwitchBranch => handle_switch_branch_key(app, key),
                ActiveOverlay::Grab => handle_grab_key(app, key),
                ActiveOverlay::Prune => handle_prune_key(app, key),
                ActiveOverlay::CherryPick => handle_cherry_pick_key(app, key),
                ActiveOverlay::History => handle_history_key(app, key),
                ActiveOverlay::ResumeSession => handle_resume_session_key(app, key),
                ActiveOverlay::RepoSelector => handle_repo_selector_key(app, key),
                ActiveOverlay::OpenRepo => handle_open_repo_key(app, key),
                ActiveOverlay::GrepSearch => handle_grep_search_key(app, key),
                ActiveOverlay::Help => handle_help_key(app, key),
                ActiveOverlay::CommandPalette => handle_command_palette_key(app, key),
                ActiveOverlay::WorktreeSwitcher => handle_worktree_key(app, key),
                ActiveOverlay::CommentList => handle_explorer_comment_list_key(app, key),
                ActiveOverlay::None => unreachable!(),
            }
            return;
        }
        EffectiveOverlay::FilenameSearch => {
            handle_filename_search_key(app, key);
            return;
        }
        EffectiveOverlay::ViewerSearch => {
            handle_viewer_search_key(app, key);
            return;
        }
        EffectiveOverlay::ReviewSearch => {
            handle_review_search_key(app, key);
            return;
        }
        EffectiveOverlay::ReviewTemplate => {
            handle_review_template_key(app, key);
            return;
        }
        EffectiveOverlay::None => {} // Fall through to panel dispatch.
    }

    // ── 1b. References overlay (panel-level popup, not part of OverlayManager) ──
    if app.references_overlay.active {
        handle_references_key(app, key);
        return;
    }

    // ── 1b2. Symbol action overlay (after hint selection) ──
    if app.symbol_action_overlay.active {
        handle_symbol_action_key(app, key);
        return;
    }

    // ── 1b3. Symbol hint overlay input (second char of label) ──
    if app.symbol_hint_overlay.active && !app.symbol_hint_overlay.input.is_empty() {
        handle_symbol_hint_key(app, key);
        return;
    }

    // ── 1c. Terminal focus — intercept configurable keys, forward rest to PTY ─

    if app.focus == Focus::TerminalClaude || app.focus == Focus::TerminalShell {
        // If the selected worktree is grabbed, block all terminal input
        // except navigation keys (focus switching is handled above in §0).
        if app.is_selected_worktree_grabbed() {
            // Allow Esc to leave terminal, but block everything else.
            if let Some(Action::LeaveTerminal) = app.keymap.resolve(&key, KeyContext::Terminal) {
                app.set_focus(Focus::Explorer);
            }
            return;
        }

        // Check terminal-specific and global bindings first.
        if let Some(action) = app.keymap.resolve(&key, KeyContext::Terminal) {
            match action {
                Action::LeaveTerminal => {
                    app.set_focus(Focus::Explorer);
                    return;
                }
                Action::FocusWorktree => {
                    app.set_focus(Focus::Worktree);
                    return;
                }
                Action::FocusExplorer => {
                    app.set_focus(Focus::Explorer);
                    return;
                }
                Action::FocusExplorerDiffList => {
                    app.set_focus(Focus::Explorer);
                    app.viewer_state.explorer.explorer_focus_on_diff_list = true;
                    return;
                }
                Action::FocusViewer => {
                    app.set_focus(Focus::Viewer);
                    return;
                }
                Action::FocusTerminalClaude => {
                    app.set_focus(Focus::TerminalClaude);
                    return;
                }
                Action::FocusTerminalShell => {
                    app.set_focus(Focus::TerminalShell);
                    return;
                }
                Action::CommandPalette => {
                    app.overlays.active = ActiveOverlay::CommandPalette;
                    app.overlays.command_palette.filter.clear();
                    app.overlays.command_palette.selected = 0;
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
                Action::OpenFileFromTerminal => {
                    terminal::open_file_from_terminal_output(app);
                    return;
                }
                Action::CycleFocusForward => {
                    app.cycle_focus_forward();
                    return;
                }
                Action::CycleFocusBackward => {
                    app.cycle_focus_backward();
                    return;
                }
                Action::NextWorktree => {
                    app.select_next_worktree();
                    return;
                }
                Action::PrevWorktree => {
                    app.select_prev_worktree();
                    return;
                }
                Action::TogglePanelOverlay => {
                    app.toggle_panel_overlay();
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

    if let Some(action) = app.keymap.resolve(&key, context)
        && dispatch_global_action(app, action)
    {
        return;
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
        let single_line: String = data.chars().filter(|c| *c != '\n' && *c != '\r').collect();

        if app.viewer_state.explorer.inline_reply_line.is_some() {
            app.viewer_state
                .explorer
                .inline_reply_buffer
                .insert_str(&single_line);
        } else if app.review_state.input_mode != ReviewInputMode::Normal {
            // Review input is multiline.
            app.review_state.input_buffer.insert_str(&data);
        } else if app.worktree_mgr.input_mode == WorktreeInputMode::SmartDescription {
            // Smart description is multiline.
            app.worktree_mgr.smart_description_buffer.insert_str(&data);
        } else if app.worktree_mgr.input_mode == WorktreeInputMode::CreatingWorktree
            || app.worktree_mgr.input_mode == WorktreeInputMode::CreatingWorktreeBase
        {
            app.worktree_mgr.input_buffer.insert_str(&single_line);
        } else if app.overlays.active == ActiveOverlay::GrepSearch {
            app.overlays.grep_search.query.insert_str(&single_line);
            app.overlays.grep_search.input_focused = true;
            app.schedule_grep_search();
        } else if app.viewer_state.search.search_active {
            app.viewer_state
                .search
                .search_query
                .insert_str(&single_line);
        } else if app.viewer_state.filename_search.filename_search_active {
            app.viewer_state
                .filename_search
                .filename_search_query
                .insert_str(&single_line);
        } else if app.review_state.search_active {
            app.review_state.search_query.insert_str(&single_line);
            app.review_state.apply_filter();
        } else {
            match app.overlays.active {
                ActiveOverlay::SwitchBranch => {
                    app.overlays.switch_branch.filter.insert_str(&single_line);
                }
                ActiveOverlay::CommandPalette => {
                    app.overlays.command_palette.filter.insert_str(&single_line);
                }
                ActiveOverlay::OpenRepo => {
                    app.overlays.open_repo.buffer.insert_str(&single_line);
                }
                ActiveOverlay::History => {
                    app.overlays.history.search_query.insert_str(&single_line);
                }
                ActiveOverlay::ResumeSession => {
                    app.overlays.resume_session.filter.insert_str(&single_line);
                }
                _ => {}
            }
        }
        return;
    }

    let session_idx = match app.focus {
        Focus::TerminalClaude => app.terminal.active_claude_session,
        Focus::TerminalShell => app.terminal.active_shell_session,
        _ => None,
    };

    // Block paste into grabbed worktree terminals.
    if app.is_selected_worktree_grabbed() {
        return;
    }

    if let Some(idx) = session_idx {
        // Use chunked write with bracketed-paste wrapping so large pastes
        // don't overflow the kernel PTY input buffer.
        if let Err(e) = app.terminal.pty_manager.write_paste_to_session(idx, &data) {
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
        .position(|&i| i == app.viewer_state.tree.tree_selected)
        .unwrap_or(0);

    let page_size = app.viewer_state.explorer.explorer_tree_height.max(1);

    if cur_vis < app.viewer_state.tree.tree_scroll {
        app.viewer_state.tree.tree_scroll = cur_vis;
    } else if cur_vis >= app.viewer_state.tree.tree_scroll + page_size {
        app.viewer_state.tree.tree_scroll = cur_vis.saturating_sub(page_size - 1);
    }
}

/// Open the fuzzy filename-search modal and seed it with the current
/// worktree's file list. Triggerable from both the Explorer (file tree) and
/// the Viewer, so files can be switched even while the viewer is maximized.
pub(super) fn open_filename_search(app: &mut App) {
    app.viewer_state.filename_search.filename_search_active = true;
    app.viewer_state.filename_search.filename_search_query.clear();
    app.viewer_state
        .filename_search
        .filename_search_results
        .clear();
    app.viewer_state.filename_search.filename_search_selected = 0;
    if let Some(wt) = app.worktrees.get(app.selected_worktree) {
        app.viewer_state.populate_filename_search_cache(&wt.path);
    }
    app.viewer_state.execute_filename_search();
}

/// Dismiss all active overlays so that focus-switching keys work globally.
fn dismiss_overlays(app: &mut App) {
    app.worktree_mgr.skip_reason = None;
    app.review_state.comment_detail_active = false;
    app.review_state.input_mode = ReviewInputMode::Normal;
    app.worktree_mgr.input_mode = WorktreeInputMode::Normal;
    app.overlays.active = ActiveOverlay::None;
    app.viewer_state.filename_search.filename_search_active = false;
    app.viewer_state.search.search_active = false;
    app.review_state.search_active = false;
    app.review_state.template_picker_active = false;
    app.references_overlay.active = false;
}

/// Adjust `diff_list_scroll` so that `diff_list_selected` stays visible.
fn adjust_diff_list_scroll(app: &mut App) {
    let selected = app.viewer_state.explorer.diff_list_selected;
    let page_size = app.viewer_state.explorer.explorer_diff_list_height.max(1);

    if selected < app.viewer_state.explorer.diff_list_scroll {
        app.viewer_state.explorer.diff_list_scroll = selected;
    } else if selected >= app.viewer_state.explorer.diff_list_scroll + page_size {
        app.viewer_state.explorer.diff_list_scroll = selected.saturating_sub(page_size - 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn bare_chars_are_printable_text() {
        // Plain ASCII, a kana, and a macOS Option-glyph fallback are all "bare"
        // and must be treated as typed text, never a global accelerator.
        for c in ['a', 'あ', '∞', '¡', '§', '÷'] {
            assert!(
                is_bare_printable(&key(KeyCode::Char(c), KeyModifiers::empty())),
                "{c:?} should be bare printable",
            );
        }
    }

    #[test]
    fn shift_only_still_counts_as_bare() {
        // Shift+a is just 'A' — still text, not a command.
        assert!(is_bare_printable(&key(
            KeyCode::Char('A'),
            KeyModifiers::SHIFT
        )));
    }

    #[test]
    fn modifier_chords_are_not_bare() {
        // Command-bearing chords must still reach the global focus switcher.
        for m in [
            KeyModifiers::CONTROL,
            KeyModifiers::ALT,
            KeyModifiers::SUPER,
            KeyModifiers::ALT | KeyModifiers::SHIFT,
        ] {
            assert!(!is_bare_printable(&key(KeyCode::Char('1'), m)));
        }
    }

    #[test]
    fn named_keys_are_not_bare_printable() {
        // Enter/Esc/Tab are not Char, so they keep flowing to global/overlay.
        for code in [KeyCode::Enter, KeyCode::Esc, KeyCode::Tab] {
            assert!(!is_bare_printable(&key(code, KeyModifiers::empty())));
        }
    }
}
