//! Viewer panel key handling.

use crossterm::event::KeyEvent;

use crate::app::App;
use crate::keymap::{Action, KeyContext};

use super::explorer::{open_viewer_comment, open_viewer_comment_detail};

/// Handle keys when the Viewer panel is focused.
pub(super) fn handle_viewer_key(app: &mut App, key: KeyEvent) {
    // Clear comment preview on any key input.
    app.viewer_state.comment_preview_line = None;

    // Unified diff mode has its own navigation.
    if app.viewer_state.diff_mode {
        handle_viewer_diff_mode_key(app, key);
        return;
    }

    let total = app.viewer_state.file_content.len();
    let action = app.keymap.resolve(&key, KeyContext::Viewer);

    if let Some(Action::ExitToExplorer) = action {
        app.viewer_state.clear_selection();
        app.set_focus(crate::app::Focus::Explorer);
        return;
    }

    if total == 0 {
        return;
    }

    match action {
        Some(Action::NavigateDown) => {
            if app.viewer_state.file_scroll + 1 < total {
                app.viewer_state.file_scroll += 1;
            }
        }
        Some(Action::NavigateUp) => {
            app.viewer_state.file_scroll = app.viewer_state.file_scroll.saturating_sub(1);
        }
        Some(Action::ScrollHalfPageDown) => {
            app.viewer_state.file_scroll =
                (app.viewer_state.file_scroll + 15).min(total.saturating_sub(1));
        }
        Some(Action::ScrollHalfPageUp) => {
            app.viewer_state.file_scroll = app.viewer_state.file_scroll.saturating_sub(15);
        }
        Some(Action::GoToTop) => {
            app.viewer_state.file_scroll = 0;
        }
        Some(Action::GoToBottom) => {
            app.viewer_state.file_scroll = total.saturating_sub(1);
        }
        Some(Action::SearchInFile) => {
            app.viewer_state.search_active = true;
            app.viewer_state.search_query.clear();
        }
        Some(Action::NextSearchMatch) => {
            app.viewer_state.next_search_match();
        }
        Some(Action::PrevSearchMatch) => {
            app.viewer_state.prev_search_match();
        }
        Some(Action::ScrollLeft) => {
            app.viewer_state.h_scroll = app.viewer_state.h_scroll.saturating_sub(4);
        }
        Some(Action::ScrollRight) => {
            app.viewer_state.scroll_right(4);
        }
        Some(Action::ScrollHome) => {
            app.viewer_state.h_scroll = 0;
        }
        Some(Action::AddComment) => {
            open_viewer_comment(app);
        }
        Some(Action::ViewCommentDetail) => {
            open_viewer_comment_detail(app);
        }
        _ => {}
    }
}

/// Key handling for the viewer panel in unified diff mode.
pub(super) fn handle_viewer_diff_mode_key(app: &mut App, key: KeyEvent) {
    let total = app.viewer_state.diff_view_lines.len();
    let action = app.keymap.resolve(&key, KeyContext::ViewerDiffMode);

    if let Some(Action::ExitToExplorer) = action {
        app.viewer_state.clear_selection();
        app.viewer_state.exit_diff_mode();
        app.set_focus(crate::app::Focus::Explorer);
        return;
    }

    if total == 0 {
        return;
    }

    match action {
        Some(Action::NavigateDown) => {
            if app.viewer_state.diff_view_scroll + 1 < total {
                app.viewer_state.diff_view_scroll += 1;
            }
        }
        Some(Action::NavigateUp) => {
            app.viewer_state.diff_view_scroll =
                app.viewer_state.diff_view_scroll.saturating_sub(1);
        }
        Some(Action::ScrollHalfPageDown) => {
            app.viewer_state.diff_view_scroll =
                (app.viewer_state.diff_view_scroll + 15).min(total.saturating_sub(1));
        }
        Some(Action::ScrollHalfPageUp) => {
            app.viewer_state.diff_view_scroll =
                app.viewer_state.diff_view_scroll.saturating_sub(15);
        }
        Some(Action::GoToTop) => {
            app.viewer_state.diff_view_scroll = 0;
        }
        Some(Action::GoToBottom) => {
            app.viewer_state.diff_view_scroll = total.saturating_sub(1);
        }
        Some(Action::ScrollLeft) => {
            app.viewer_state.h_scroll = app.viewer_state.h_scroll.saturating_sub(4);
        }
        Some(Action::ScrollRight) => {
            app.viewer_state.scroll_right(4);
        }
        Some(Action::ScrollHome) => {
            app.viewer_state.h_scroll = 0;
        }
        Some(Action::AddComment) => {
            if let Some(entry) = app.viewer_state.diff_view_lines.get(app.viewer_state.diff_view_scroll) {
                match entry {
                    crate::viewer::UnifiedDiffEntry::Line { tag, new_line_no: Some(_), .. }
                        if *tag != crate::diff_state::DiffLineTag::Delete =>
                    {
                        open_viewer_comment(app);
                    }
                    _ => {
                        app.status_message = Some("Cannot comment on deleted lines".to_string().into());
                    }
                }
            }
        }
        Some(Action::ViewCommentDetail) => {
            open_viewer_comment_detail(app);
        }
        _ => {}
    }
}
