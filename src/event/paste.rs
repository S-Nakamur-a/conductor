//! Bracketed-paste event handling.

use crate::app::{App, Focus, WorktreeInputMode};
use crate::overlay::ActiveOverlay;
use crate::review_state::ReviewInputMode;

use super::is_text_input_active;

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
                ActiveOverlay::PrInput => {
                    app.overlays.pr_input.buffer.insert_str(&single_line);
                    app.overlays.pr_input.error = None;
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
