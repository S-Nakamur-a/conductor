//! Explorer のトップレベルキー処理: ファイルツリーのナビゲーションと、
//! diff 一覧・コメント一覧の各サブパネルへの委譲。

use crossterm::event::KeyEvent;

use crate::app::{App, Focus};
use crate::keymap::{Action, KeyContext};
use crate::viewer::ExplorerBottomView;

use super::comment_list::handle_explorer_comment_list_key;
use super::diff_list::handle_explorer_diff_list_key;

/// Explorer パネルがフォーカスされているときのキーを処理する。
pub(in crate::event) fn handle_explorer_key(app: &mut App, key: KeyEvent) -> Option<KeyEvent> {
    if app.explorer.tree.file_tree.is_empty() {
        app.refresh_viewer();
    }

    // サブパネルへ委譲する前に show-diff / show-comments をチェックする。
    let action = app.keymap.resolve(&key, KeyContext::Explorer);
    match action {
        Some(Action::ShowDiffList) => {
            app.explorer.bottom_view = ExplorerBottomView::DiffList;
            app.explorer.focus_on_diff_list = true;
            return None;
        }
        Some(Action::ShowCommentList) => {
            app.explorer.bottom_view = ExplorerBottomView::Comments;
            app.explorer.focus_on_diff_list = true;
            return None;
        }
        _ => {}
    }

    if app.explorer.focus_on_diff_list {
        return match app.explorer.bottom_view {
            ExplorerBottomView::Comments => handle_explorer_comment_list_key(app, key),
            ExplorerBottomView::DiffList => handle_explorer_diff_list_key(app, key),
        };
    }

    let visible = app.explorer.visible_indices();
    if visible.is_empty() {
        return None;
    }

    let cur_vis = visible
        .iter()
        .position(|&i| i == app.explorer.tree.tree_selected)
        .unwrap_or(0);

    match action {
        Some(Action::NavigateDown) if cur_vis + 1 < visible.len() => {
            app.explorer.tree.tree_selected = visible[cur_vis + 1];
        }
        Some(Action::NavigateUp) if cur_vis > 0 => {
            app.explorer.tree.tree_selected = visible[cur_vis - 1];
        }
        Some(Action::Select) => {
            let idx = app.explorer.tree.tree_selected;
            if let Some(entry) = app.explorer.tree.file_tree.get(idx).cloned() {
                if entry.is_dir {
                    if !entry.is_expanded {
                        app.explorer.ensure_children_loaded(idx);
                    }
                    app.explorer.toggle_dir(idx);
                } else {
                    let tab_width = app.config.viewer.tab_width;
                    app.viewer
                        .open_file(app.explorer.root(), &entry.path, tab_width);
                    app.rehighlight_viewer();
                    app.review_state.build_file_comment_cache(&entry.path);
                    app.set_focus(Focus::Viewer);
                }
            }
        }
        Some(Action::ExpandOrRight) => {
            let idx = app.explorer.tree.tree_selected;
            let needs_children = app
                .explorer
                .tree
                .file_tree
                .get(idx)
                .is_some_and(|e| e.is_dir && !e.is_expanded);
            if needs_children {
                app.explorer.ensure_children_loaded(idx);
            }
            app.explorer.expand_dir(idx);
        }
        Some(Action::CollapseOrLeft) => {
            let idx = app.explorer.tree.tree_selected;
            app.explorer.collapse_dir(idx);
        }
        Some(Action::GoToTop) => {
            if let Some(&first) = visible.first() {
                app.explorer.tree.tree_selected = first;
            }
        }
        Some(Action::GoToBottom) => {
            if let Some(&last) = visible.last() {
                app.explorer.tree.tree_selected = last;
            }
        }
        Some(Action::SearchFilename) => {
            crate::event::open_filename_search(app);
        }
        _ => {}
    }

    crate::event::adjust_tree_scroll(app);
    None
}
