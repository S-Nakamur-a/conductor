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
pub mod reflow;
mod terminal;
mod viewer;
mod worktree;

use crossterm::event::{KeyCode, KeyEvent};

use crate::app::{App, Focus, UpdateState, WorktreeInputMode};
use crate::keymap::{Action, KeyContext};
use crate::overlay::ActiveOverlay;
use crate::review_state::ReviewInputMode;

use self::explorer::handle_explorer_comment_list_key;
use self::explorer::handle_explorer_key;
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

// Re-export public API.
pub use self::mouse::handle_mouse_event;

/// Process a single key event, updating application state as needed.
pub fn handle_key_event(app: &mut App, key: KeyEvent) {
    // ── 0. Global focus-switching — available over non-text overlays (y/n
    // confirms, list pickers), but NOT while a text field is focused. A focused
    // text field is a modal grab: it owns every key, so the global focus layer
    // is not consulted and chords can't pierce it (press Esc to leave first).
    //
    // This is what stops focus theft mid-IME-input. Under the kitty keyboard
    // protocol, macOS reports Option-composed input with the ALT bit set, so a
    // composed glyph ('∞' → Claude panel) or a Meta-mode digit ('alt+5') would
    // otherwise resolve to a focus action and yank focus away while typing.
    // Grabbing on `is_text_input_active` closes the whole category at once,
    // without enumerating which key shapes a terminal might emit. Non-text
    // overlays stay pierceable because `is_text_input_active` is false for them.
    if !is_text_input_active(app)
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
                ActiveOverlay::ThemePicker => handle_theme_picker_key(app, key),
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

    // ── 1b4. Reflow transcript view — consume all keys while active ──────────
    // Must sit before the PTY-forward path so keys are not forwarded to Claude.
    if app.reflow.active && app.focus == Focus::TerminalClaude {
        handle_reflow_key(app, key);
        return;
    }

    // ── 1c. PTY focus — intercept configurable keys, forward rest to PTY ─
    // Covers the Claude/Shell terminals and the embedded editor: each forwards
    // unstolen keys to its inner program, using its own keymap context.

    if app.focus.is_pty() {
        let pty_context = app.focus.key_context();

        // If the selected worktree is grabbed, block all terminal input
        // except navigation keys (focus switching is handled above in §0).
        // (The editor never opens on a grabbed worktree, so this only guards
        // the Claude/Shell terminals in practice.)
        if app.is_selected_worktree_grabbed() {
            // Allow Esc to leave terminal, but block everything else.
            if let Some(Action::LeaveTerminal) = app.keymap.resolve(&key, pty_context) {
                app.set_focus(Focus::Explorer);
            }
            return;
        }

        // Resolve via the keymap (panel layer + global fallback, already
        // filtered to actions that fire in the terminal). Terminal-only actions
        // need terminal state; everything else reuses the shared global
        // dispatch. A miss falls through to the PTY below — the keymap is the
        // single source of truth for what the panel steals from the inner
        // program (no hand-maintained allowlist).
        if let Some(action) = app.keymap.resolve(&key, pty_context)
            && (handle_terminal_only_action(app, action) || dispatch_global_action(app, action))
        {
            return;
        }

        // Forward all remaining keys to the active PTY session.
        let session_idx = match app.focus {
            Focus::TerminalClaude => app.terminal.active_claude_session,
            Focus::TerminalShell => app.terminal.active_shell_session,
            Focus::Editor => app.editor.as_ref().map(|e| e.session_idx),
            _ => unreachable!(),
        };
        if let Some(idx) = session_idx {
            forward_key_to_pty(app, idx, key);
        } else if key.code == KeyCode::Enter && app.focus != Focus::Editor {
            spawn_terminal_session(app);
        }
        return;
    }

    // ── 2. Non-terminal panels — resolve via keymap ──────────────────

    let context = match app.focus {
        Focus::Worktree => KeyContext::Worktree,
        Focus::Explorer => KeyContext::Explorer,
        Focus::Viewer => KeyContext::Viewer,
        Focus::TerminalClaude | Focus::TerminalShell | Focus::Editor => unreachable!(),
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
        Focus::TerminalClaude | Focus::TerminalShell | Focus::Editor => unreachable!(),
    }
}

/// Handle the terminal-only actions (scrollback, leave, open-file) that need
/// terminal state and have no meaning in any other panel, so they cannot route
/// through `dispatch_global_action`. Returns `true` if handled; `false` for any
/// other action (which the caller then sends to `dispatch_global_action`).
/// Only called while a terminal panel is focused, so the `unreachable!()` arms
/// hold.
fn handle_terminal_only_action(app: &mut App, action: Action) -> bool {
    match action {
        Action::LeaveTerminal => {
            // While the editor panel is open, Ctrl+Esc toggles between it and
            // Claude (the editor stays open so you can chat and return); from
            // the editor itself it steps over to Claude. Otherwise it leaves a
            // terminal back to the Explorer as before.
            let target = if app.editor.is_some() {
                match app.focus {
                    Focus::Editor => Focus::TerminalClaude,
                    _ => Focus::Editor,
                }
            } else {
                Focus::Explorer
            };
            app.set_focus(target);
        }
        Action::ScrollbackUp => {
            // Intercept the first upward scroll on the live Claude terminal
            // (scroll_claude == 0) to enter the infinite-scrollback reflow view
            // instead of the limited vt100 scrollback buffer.
            if app.focus == Focus::TerminalClaude
                && app.terminal.scroll_claude == 0
                && !app.reflow.active
            {
                app.open_reflow();
                return true;
            }
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
        }
        Action::ScrollbackTop => {
            // Same hijack as ScrollbackUp: jump straight to reflow on Claude live view.
            if app.focus == Focus::TerminalClaude
                && app.terminal.scroll_claude == 0
                && !app.reflow.active
            {
                app.open_reflow();
                return true;
            }
            match app.focus {
                Focus::TerminalClaude => app.terminal.scroll_claude = 1000,
                Focus::TerminalShell => app.terminal.scroll_shell = 1000,
                _ => unreachable!(),
            }
        }
        Action::SnapToLive => match app.focus {
            Focus::TerminalClaude => app.terminal.scroll_claude = 0,
            Focus::TerminalShell => app.terminal.scroll_shell = 0,
            _ => unreachable!(),
        },
        Action::OpenFileFromTerminal => terminal::open_file_from_terminal_output(app),
        _ => return false,
    }
    true
}

// ── Reflow key handling ─────────────────────────────────────────────────

/// Handle a key event while the reflow transcript view is active.
///
/// All keys are consumed here and never forwarded to the PTY — the reflow
/// view is a pure read-only overlay. Navigation:
///
/// * `j` / Down — scroll down one line.
/// * `k` / Up — scroll up one line.
/// * `Ctrl-d` / PageDown — scroll down half a page.
/// * `Ctrl-u` / PageUp — scroll up half a page.
/// * `g` / Home — jump to the oldest turn (top).
/// * `G` / End — jump to the newest turn (bottom) without leaving.
/// * `Esc` — close reflow view and return to live PTY.
/// * `j` / Down / PageDown at the bottom — close reflow (live return).
fn handle_reflow_key(app: &mut App, key: KeyEvent) {
    use crossterm::event::{KeyCode, KeyModifiers};
    use crate::event::reflow::{at_bottom, clamp_scroll};

    let inner = app.reflow.last_inner_height as usize;
    let total = app.reflow.total_lines;
    let page: usize = (inner / 2).max(1);
    let bottom = at_bottom(app.reflow.scroll, total, inner);

    match key.code {
        // ── Line scroll ─────────────────────────────────────────────────────
        KeyCode::Char('j') | KeyCode::Down => {
            if bottom {
                // Bottom + discrete down-key → begin exit sweep back to live PTY.
                app.request_close_reflow();
                return;
            }
            app.reflow.scroll = app.reflow.scroll.saturating_add(1);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.reflow.scroll = app.reflow.scroll.saturating_sub(1);
        }

        // ── Page scroll ──────────────────────────────────────────────────────
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if bottom {
                app.request_close_reflow();
                return;
            }
            app.reflow.scroll = app.reflow.scroll.saturating_add(page);
        }
        KeyCode::PageDown => {
            if bottom {
                app.request_close_reflow();
                return;
            }
            app.reflow.scroll = app.reflow.scroll.saturating_add(page);
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.reflow.scroll = app.reflow.scroll.saturating_sub(page);
        }
        KeyCode::PageUp => {
            app.reflow.scroll = app.reflow.scroll.saturating_sub(page);
        }

        // ── Jump to top / bottom ─────────────────────────────────────────────
        KeyCode::Char('g') | KeyCode::Home => {
            app.reflow.scroll = 0;
        }
        KeyCode::Char('G') | KeyCode::End => {
            // Snap to the newest turn (logical bottom) without leaving the view.
            app.reflow.scroll = total.saturating_sub(inner);
        }

        // ── Leave ────────────────────────────────────────────────────────────
        KeyCode::Esc => {
            // Play the exit sweep before returning to the live PTY.
            app.request_close_reflow();
            return;
        }

        _ => {} // All other keys are silently consumed.
    }

    // Clamp scroll after any adjustment.  Upper bound is total - inner, not
    // total - 1: aligns with the render path and at_bottom logic.
    app.reflow.scroll = clamp_scroll(app.reflow.scroll, total, inner);
}

// ── Paste event handling ────────────────────────────────────────────────

/// Handle a bracketed paste event. A text-input overlay/modal takes the paste
/// first (so IME-committed multi-byte text reaches the input field rather than a
/// terminal sitting behind the modal); otherwise, when a terminal panel is
/// focused, the entire pasted text is forwarded to the PTY in one write, wrapped
/// with bracketed-paste escape sequences so the shell/application treats it as a
/// single paste rather than individual keystrokes.
pub fn handle_paste_event(app: &mut App, data: String) {
    // A text-input overlay/modal owns paste regardless of which panel holds
    // focus underneath it — the same modal grab that §0 of `handle_key_event`
    // applies to key events. This matters because macOS terminals deliver
    // IME-committed multi-byte text (kana/kanji, especially 2+ chars or a
    // conversion) as a bracketed paste, not as individual key events. Gating on
    // focus alone would forward that paste into the focused Claude/Shell PTY
    // sitting behind the modal, so typed Japanese would vanish from the input
    // field and surface in the terminal instead. Half-width ASCII is unaffected
    // because it arrives as ordinary key events. Kept in lockstep with
    // `is_text_input_active`: every destination below is enumerated there.
    if is_text_input_active(app) {
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
    app.viewer_state
        .filename_search
        .filename_search_query
        .clear();
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

// NOTE: the focus-grab decision now lives in `is_text_input_active` (a function
// of `App` state) gating §0 of `handle_key_event`. There is no cheap pure-fn
// seam to unit-test it in isolation here — `App::new` does real git work — so it
// is covered by manual/integration testing rather than a unit test in this file.
