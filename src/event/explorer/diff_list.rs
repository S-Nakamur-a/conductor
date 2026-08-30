//! Explorer diff-list サブパネル: 統合 diff リスト（ファイル、ディレクトリ、
//! セクションヘッダ）のナビゲーションと操作。

use crossterm::event::KeyEvent;

use crate::app::{App, Focus};
use crate::keymap::{Action, KeyContext};

pub(super) fn handle_explorer_diff_list_key(app: &mut App, key: KeyEvent) -> Option<KeyEvent> {
    let count = app.diff_state.display_list.len();
    let action = app.keymap.resolve(&key, KeyContext::ExplorerDiffList);

    match action {
        Some(Action::ExitSubPanel) => {
            app.explorer.focus_on_diff_list = false;
        }
        Some(Action::NavigateDown) if count > 0 && app.explorer.diff_list_selected + 1 < count => {
            app.explorer.diff_list_selected += 1;
        }
        Some(Action::NavigateUp) if app.explorer.diff_list_selected > 0 => {
            app.explorer.diff_list_selected -= 1;
        }
        Some(Action::CollapseOrLeft) => {
            let selected = app.explorer.diff_list_selected;
            app.diff_state.collapse_section(selected);
            let new_count = app.diff_state.display_list.len();
            if new_count > 0 && app.explorer.diff_list_selected >= new_count {
                app.explorer.diff_list_selected = new_count - 1;
            }
        }
        Some(Action::ExpandOrRight) => {
            let selected = app.explorer.diff_list_selected;
            app.diff_state.expand_section(selected);
        }
        Some(Action::Select) => {
            let selected = app.explorer.diff_list_selected;
            // SUMMARY 疑似ファイルはブランチの変更サマリをフルパネルで開く。
            if matches!(
                app.diff_state.display_list.get(selected),
                Some(crate::diff_state::DiffListEntry::Summary {})
            ) {
                app.viewer.enter_summary_view();
                app.set_focus(Focus::Viewer);
            }
            // Enter でセクションヘッダとディレクトリの開閉を切り替える。
            else if app.diff_state.toggle_section(selected) {
                let new_count = app.diff_state.display_list.len();
                if new_count > 0 && app.explorer.diff_list_selected >= new_count {
                    app.explorer.diff_list_selected = new_count - 1;
                }
            } else if app.diff_state.resolve_file(selected).is_some() {
                // diff_list_selected は既にこの行を指している。
                app.open_diff_file_at_selected();
                app.set_focus(Focus::Viewer);
            }
        }
        Some(Action::GoToTop) => {
            app.explorer.diff_list_selected = 0;
        }
        Some(Action::GoToBottom) if count > 0 => {
            app.explorer.diff_list_selected = count - 1;
        }
        Some(Action::ToggleViewed) => {
            let selected = app.explorer.diff_list_selected;
            if let Some(file_diff) = app.diff_state.resolve_file(selected) {
                let path = file_diff.path.clone();
                app.toggle_path_viewed(&path);
            }
        }
        _ => {}
    }

    crate::event::adjust_diff_list_scroll(app);
    None
}
