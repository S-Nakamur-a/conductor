//! Event handling — maps keyboard and mouse events to application actions.
//!
//! Focus-based dispatching: Tab / Shift+Tab cycle between non-terminal panels;
//! Alt+h / Alt+l cycle between all panels including terminals.
//! Overlay handlers (worktree input, cherry-pick, etc.) take priority.
//! Terminal-focused panels forward keys to the active PTY session.

mod clipboard;
mod dialogs;
mod explorer;
mod explorer_walkthrough;
mod global;
mod mouse;
mod overlay;
mod overlay_helpers;
mod paste;
pub mod reflow;
mod reflow_key;
mod scroll;
mod terminal;
mod viewer;
mod worktree;

use crossterm::event::{KeyCode, KeyEvent};

use crate::app::{App, Focus, UpdateState, WorktreeInputMode};
use crate::keymap::{Action, KeyContext};
use crate::overlay::ActiveOverlay;
use crate::review_state::ReviewInputMode;

use self::dialogs::{handle_publish_confirm_key, handle_update_key};
use self::explorer::handle_explorer_comment_list_key;
use self::explorer::handle_explorer_key;
use self::global::dispatch_global_action;
use self::overlay::*;
use self::overlay_helpers::dismiss_overlays;
use self::reflow_key::handle_reflow_key;
use self::terminal::{forward_key_to_pty, spawn_terminal_session};
use self::viewer::handle_viewer_key;
use self::worktree::handle_worktree_key;

// Re-export items whose original path was `crate::event::X` but which now
// live in a sibling submodule, so sibling modules' existing `super::X`
// references keep resolving without modification.
pub(in crate::event) use self::clipboard::clipboard_paste;
pub(in crate::event) use self::overlay_helpers::open_filename_search;
pub(in crate::event) use self::scroll::{
    adjust_diff_list_scroll, adjust_tree_scroll, adjust_walkthrough_scroll,
};

// ── Effective overlay ───────────────────────────────────────────────────

/// Unified overlay/modal state for dispatch. Collapses the multiple
/// boolean/enum checks into a single discriminant.
enum EffectiveOverlay {
    /// Skip-reason modal (worktree creation failure detail).
    SkipReason,
    /// Update confirmation/progress/failure dialog.
    UpdateState,
    /// Publish-to-GitHub confirmation dialog.
    PublishConfirm,
    /// Comment detail popup.
    CommentDetail,
    /// Walkthrough step detail popup (Explorer walkthrough view's `space`).
    WalkthroughDetail,
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
    if app.publish_confirm.is_some() {
        return EffectiveOverlay::PublishConfirm;
    }
    if app.review_state.comment_detail_active {
        return EffectiveOverlay::CommentDetail;
    }
    if app.viewer_state.explorer.walkthrough_detail_active {
        return EffectiveOverlay::WalkthroughDetail;
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
            | ActiveOverlay::PrInput
            | ActiveOverlay::History
            | ActiveOverlay::ResumeSession
    )
}

// Re-export public API.
pub use self::mouse::handle_mouse_event;
pub use self::paste::handle_paste_event;

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
        EffectiveOverlay::PublishConfirm => {
            handle_publish_confirm_key(app, key);
            return;
        }
        EffectiveOverlay::CommentDetail => {
            handle_comment_detail_key(app, key);
            return;
        }
        EffectiveOverlay::WalkthroughDetail => {
            if matches!(
                key.code,
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char(' ')
            ) {
                app.viewer_state.explorer.walkthrough_detail_active = false;
            }
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
                ActiveOverlay::PrInput => handle_pr_input_key(app, key),
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

    // ── 1b1. Hover modal. When pinned (the user clicked into it), keys drive
    // the modal stack — Esc pops a level, arrows navigate the refs list, Enter
    // drills in / jumps — and are consumed. Otherwise it's a transient auto
    // popup: any key dismisses it (Esc consumed; other keys fall through to do
    // their normal job as the popup vanishes). ──
    if app.hover_info_overlay.pinned {
        handle_hover_modal_key(app, key);
        return;
    }
    if app.hover_info_overlay.info.is_some() || app.hover_info_overlay.pending.is_some() {
        app.clear_hover();
        if key.code == KeyCode::Esc {
            return;
        }
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

    // ── 1b4/1c. Reflow transcript view and PTY focus ──────────────────────
    // Must sit before the Focus dispatch below so keys are not forwarded to
    // Claude. Factored into `dispatch_pty_key` so both the reflow-over-Claude
    // case and the plain PTY-focus case share one code path.
    if (app.reflow.active && app.focus == Focus::TerminalClaude) || app.focus.is_pty() {
        dispatch_pty_key(app, key);
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

/// Keyboard driver for the pinned interactive hover modal stack. Esc pops the
/// deepest open level (preview → refs list → the whole popup); Up/Down (or k/j)
/// move the references selection; Enter opens the selected reference's preview,
/// or — when a preview is already showing — jumps to that location.
fn handle_hover_modal_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.hover_pop_level();
        }
        KeyCode::Up | KeyCode::Char('k') => app.hover_refs_move(-1),
        KeyCode::Down | KeyCode::Char('j') => app.hover_refs_move(1),
        KeyCode::Enter => {
            let has_preview = app
                .hover_info_overlay
                .refs
                .as_ref()
                .is_some_and(|r| r.preview.is_some());
            if has_preview {
                app.hover_jump_to_preview();
            } else if let Some(sel) =
                app.hover_info_overlay.refs.as_ref().map(|r| r.selected)
            {
                app.open_hover_preview(sel);
            }
        }
        _ => {}
    }
}

/// Dispatch a key event for a PTY-focused panel (Claude/Shell/Editor), or for
/// the reflow transcript view layered over the Claude terminal. Callers must
/// only invoke this when `app.focus.is_pty()` or the reflow-over-Claude
/// condition holds — `handle_key_event`'s panel dispatch guarantees this.
fn dispatch_pty_key(app: &mut App, key: KeyEvent) {
    // Reflow transcript view — consume all keys while active. Pane
    // resize/zoom/panel-overlay still work while scrolled back — they don't
    // conflict with reflow's plain-key navigation (j/k/arrows), so let those
    // chords (Ctrl+Alt+Arrow, etc.) through instead of letting reflow
    // silently swallow them.
    if app.reflow.active && app.focus == Focus::TerminalClaude {
        if let Some(action) = app.keymap.resolve(&key, KeyContext::Terminal)
            && matches!(
                action,
                Action::ResizePaneLeft
                    | Action::ResizePaneRight
                    | Action::ResizePaneUp
                    | Action::ResizePaneDown
                    | Action::TogglePanelExpand
                    | Action::TogglePanelOverlay
            )
            && dispatch_global_action(app, action)
        {
            return;
        }
        handle_reflow_key(app, key);
        return;
    }

    if !app.focus.is_pty() {
        return;
    }
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

    // Courtesy hint: Ctrl+Q is Conductor's quit chord, but in a terminal it's
    // forwarded to the inner program (XON / flow-control), so a user pressing
    // it here gets no quit. Flash how to actually quit, then forward as usual.
    if key.code == KeyCode::Char('q')
        && key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
    {
        app.set_status_info(
            "Ctrl+Q is sent to the terminal here. To quit Conductor: Ctrl+Esc to leave, then Ctrl+Q.".to_string(),
        );
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
        Action::NextSession => app.cycle_terminal_session(true),
        Action::PrevSession => app.cycle_terminal_session(false),
        _ => return false,
    }
    true
}

// NOTE: the focus-grab decision now lives in `is_text_input_active` (a function
// of `App` state) gating §0 of `handle_key_event`. There is no cheap pure-fn
// seam to unit-test it in isolation here — `App::new` does real git work — so it
// is covered by manual/integration testing rather than a unit test in this file.
