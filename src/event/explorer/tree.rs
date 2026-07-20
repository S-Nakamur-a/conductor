//! Top-level Explorer key handling: file tree navigation, and delegating to
//! the diff-list / comment-list / walkthrough sub-panels.

use crossterm::event::KeyEvent;

use crate::app::{App, Focus};
use crate::keymap::{Action, KeyContext};
use crate::viewer::ExplorerBottomView;

use super::comment_list::handle_explorer_comment_list_key;
use super::diff_list::handle_explorer_diff_list_key;

/// Handle keys when the Explorer panel is focused.
pub(in crate::event) fn handle_explorer_key(app: &mut App, key: KeyEvent) {
    if app.viewer_state.tree.file_tree.is_empty() {
        app.refresh_viewer();
    }

    // Check for show-diff / show-comments / show-walkthrough before delegating
    // to sub-panels.
    let action = app.keymap.resolve(&key, KeyContext::Explorer);
    match action {
        Some(Action::ShowDiffList) => {
            app.viewer_state.explorer.explorer_bottom_view = ExplorerBottomView::DiffList;
            app.viewer_state.explorer.explorer_focus_on_diff_list = true;
            return;
        }
        Some(Action::ShowCommentList) => {
            app.viewer_state.explorer.explorer_bottom_view = ExplorerBottomView::Comments;
            app.viewer_state.explorer.explorer_focus_on_diff_list = true;
            return;
        }
        Some(Action::ShowWalkthrough) => {
            app.viewer_state.explorer.explorer_bottom_view = ExplorerBottomView::Walkthrough;
            app.viewer_state.explorer.explorer_focus_on_diff_list = true;
            return;
        }
        _ => {}
    }

    if app.viewer_state.explorer.explorer_focus_on_diff_list {
        match app.viewer_state.explorer.explorer_bottom_view {
            ExplorerBottomView::Comments => handle_explorer_comment_list_key(app, key),
            ExplorerBottomView::Walkthrough => {
                crate::event::explorer_walkthrough::handle_explorer_walkthrough_key(app, key)
            }
            ExplorerBottomView::DiffList => handle_explorer_diff_list_key(app, key),
        }
        return;
    }

    let visible = app.viewer_state.visible_indices();
    if visible.is_empty() {
        return;
    }

    let cur_vis = visible
        .iter()
        .position(|&i| i == app.viewer_state.tree.tree_selected)
        .unwrap_or(0);

    match action {
        Some(Action::NavigateDown) if cur_vis + 1 < visible.len() => {
            app.viewer_state.tree.tree_selected = visible[cur_vis + 1];
        }
        Some(Action::NavigateUp) if cur_vis > 0 => {
            app.viewer_state.tree.tree_selected = visible[cur_vis - 1];
        }
        Some(Action::Select) => {
            let idx = app.viewer_state.tree.tree_selected;
            if let Some(entry) = app.viewer_state.tree.file_tree.get(idx).cloned() {
                if entry.is_dir {
                    if !entry.is_expanded
                        && let Some(wt) = app.worktrees.get(app.selected_worktree)
                    {
                        app.viewer_state.ensure_children_loaded(idx, &wt.path);
                    }
                    app.viewer_state.toggle_dir(idx);
                } else if let Some(wt) = app.worktrees.get(app.selected_worktree) {
                    let path = wt.path.clone();
                    let tab_width = app.config.viewer.tab_width;
                    app.viewer_state.open_file(&path, &entry.path, tab_width);
                    app.rehighlight_viewer();
                    app.review_state.build_file_comment_cache(&entry.path);
                    app.set_focus(Focus::Viewer);
                }
            }
        }
        Some(Action::ExpandOrRight) => {
            let idx = app.viewer_state.tree.tree_selected;
            if let Some(entry) = app.viewer_state.tree.file_tree.get(idx)
                && entry.is_dir
                && !entry.is_expanded
                && let Some(wt) = app.worktrees.get(app.selected_worktree)
            {
                app.viewer_state.ensure_children_loaded(idx, &wt.path);
            }
            app.viewer_state.expand_dir(idx);
        }
        Some(Action::CollapseOrLeft) => {
            let idx = app.viewer_state.tree.tree_selected;
            app.viewer_state.collapse_dir(idx);
        }
        Some(Action::GoToTop) => {
            if let Some(&first) = visible.first() {
                app.viewer_state.tree.tree_selected = first;
            }
        }
        Some(Action::GoToBottom) => {
            if let Some(&last) = visible.last() {
                app.viewer_state.tree.tree_selected = last;
            }
        }
        Some(Action::SearchFilename) => {
            crate::event::open_filename_search(app);
        }
        _ => {}
    }

    crate::event::adjust_tree_scroll(app);
}
