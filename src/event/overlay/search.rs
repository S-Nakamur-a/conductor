//! 検索系オーバーレイ: ファイル名（あいまいファイルファインダ）、grep
//! （プロジェクト全体の全文検索）、viewer 内テキスト検索。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, Focus};
use crate::keymap::{Action, KeyContext};
use crate::overlay::ActiveOverlay;

use crate::event::clipboard_paste;

use super::filterable_overlay_list_nav;

// オーバーレイ: ファイル名検索

pub(in crate::event) fn handle_filename_search_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.viewer_state.filename_search.filename_search_active = false;
            app.viewer_state
                .filename_search
                .filename_search_query
                .clear();
            app.viewer_state
                .filename_search
                .filename_search_results
                .clear();
            app.viewer_state.filename_search.filename_search_selected = 0;
        }
        KeyCode::Enter => {
            if let Some(result) = app
                .viewer_state
                .filename_search
                .filename_search_results
                .get(app.viewer_state.filename_search.filename_search_selected)
                .cloned()
            {
                app.viewer_state.filename_search.filename_search_active = false;

                // 選択したファイルをツリー上に表示して開く（Focus は Explorer のまま）。
                app.viewer_state.reveal_file_in_tree(&result.path);
                let tab_width = app.config.viewer.tab_width;
                app.viewer_state.open_file(&result.path, tab_width);
                app.rehighlight_viewer();
                app.review_state.build_file_comment_cache(&result.path);
            }
            app.viewer_state
                .filename_search
                .filename_search_query
                .clear();
            app.viewer_state
                .filename_search
                .filename_search_results
                .clear();
            app.viewer_state.filename_search.filename_search_selected = 0;
        }
        KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
            app.viewer_state
                .filename_search
                .filename_search_query
                .delete_to_line_start();
            app.viewer_state.filename_search.filename_search_selected = 0;
        }
        _ if filterable_overlay_list_nav(
            &app.keymap,
            &key,
            &mut app.viewer_state.filename_search.filename_search_selected,
            app.viewer_state
                .filename_search
                .filename_search_results
                .len(),
        ) => {}
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let count = app
                .viewer_state
                .filename_search
                .filename_search_results
                .len();
            if count > 0 && app.viewer_state.filename_search.filename_search_selected + 1 < count {
                app.viewer_state.filename_search.filename_search_selected += 1;
            }
        }
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if app.viewer_state.filename_search.filename_search_selected > 0 {
                app.viewer_state.filename_search.filename_search_selected -= 1;
            }
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            clipboard_paste(
                app,
                |a| &mut a.viewer_state.filename_search.filename_search_query,
                false,
            );
            app.viewer_state.filename_search.filename_search_selected = 0;
            app.viewer_state.execute_filename_search();
        }
        _ => {
            if app
                .viewer_state
                .filename_search
                .filename_search_query
                .handle_key(key)
            {
                // テキストを変更するキーは選択をリセットして検索を再実行する。
                match key.code {
                    KeyCode::Backspace | KeyCode::Delete | KeyCode::Char(_) => {
                        app.viewer_state.filename_search.filename_search_selected = 0;
                        app.viewer_state.execute_filename_search();
                    }
                    _ => {}
                }
            }
        }
    }
}

// オーバーレイ: grep（全文）検索

pub(in crate::event) fn handle_grep_search_key(app: &mut App, key: KeyEvent) {
    use crate::search_result_tree::SearchTreeRow;

    // 入力/結果どちらにフォーカスがあっても処理するキー
    match key.code {
        KeyCode::Esc => {
            if !app.overlays.grep_search.input_focused {
                // 閉じずに入力フィールドへフォーカスを戻す。
                app.overlays.grep_search.input_focused = true;
            } else {
                app.overlays.active = ActiveOverlay::None;
                app.overlays.grep_search.running = false;
                app.overlays.grep_search.bg_op.clear();
                app.overlays.grep_search.bg_op_phase2.clear();
                app.overlays.grep_search.debounce_deadline = None;
                app.overlays.grep_search.phase1_active = false;
            }
            return;
        }
        KeyCode::Tab | KeyCode::BackTab => {
            app.overlays.grep_search.input_focused = !app.overlays.grep_search.input_focused;
            return;
        }
        // Ctrl+r / Ctrl+i / Ctrl+v / Cmd+Backspace — 常に利用可能。
        KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.overlays.grep_search.regex_mode = !app.overlays.grep_search.regex_mode;
            app.schedule_grep_search();
            return;
        }
        KeyCode::Char('i') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.overlays.grep_search.case_sensitive = !app.overlays.grep_search.case_sensitive;
            app.schedule_grep_search();
            return;
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            clipboard_paste(app, |a| &mut a.overlays.grep_search.query, false);
            app.overlays.grep_search.input_focused = true;
            app.schedule_grep_search();
            return;
        }
        KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
            app.overlays.grep_search.query.delete_to_line_start();
            app.overlays.grep_search.input_focused = true;
            app.schedule_grep_search();
            return;
        }
        // 入力欄で下矢印を押すとフォーカスが結果側に移る。
        KeyCode::Down if app.overlays.grep_search.input_focused => {
            app.overlays.grep_search.input_focused = false;
            return;
        }
        // Enter — 結果へジャンプするか展開を切り替える（両モードで動作）。
        KeyCode::Enter => {
            let selected = app.overlays.grep_search.selected;
            let result = app
                .overlays
                .grep_search
                .result_tree
                .get_match_at(selected)
                .cloned();
            if let Some(result) = result {
                app.overlays.active = ActiveOverlay::None;
                app.overlays.grep_search.running = false;
                app.overlays.grep_search.bg_op.clear();
                app.overlays.grep_search.bg_op_phase2.clear();
                app.overlays.grep_search.debounce_deadline = None;
                app.overlays.grep_search.phase1_active = false;

                app.viewer_state.reveal_file_in_tree(&result.file_path);
                let tab_width = app.config.viewer.tab_width;
                app.viewer_state.open_file(&result.file_path, tab_width);
                app.rehighlight_viewer();
                let hit_0 = result.line_number.saturating_sub(1);
                let max = app
                    .viewer_state
                    .content
                    .file_content
                    .len()
                    .saturating_sub(1);
                app.viewer_state.content.file_scroll =
                    result.line_number.saturating_sub(6).min(max);
                app.viewer_state.content.grep_highlight_line = Some(result.line_number);
                if app.viewer_state.content.file_scroll > hit_0 {
                    app.viewer_state.content.file_scroll = hit_0;
                }
                app.viewer_state.show_raw_for_line_target();
                app.set_focus(Focus::Viewer);
            } else {
                app.overlays.grep_search.result_tree.toggle_expand(selected);
            }
            return;
        }
        _ => {}
    }

    // 入力フォーカスモード: すべてのキーがテキスト入力に送られる
    if app.overlays.grep_search.input_focused {
        if app.overlays.grep_search.query.handle_key(key) {
            match key.code {
                KeyCode::Backspace | KeyCode::Delete | KeyCode::Char(_) => {
                    app.schedule_grep_search();
                }
                _ => {}
            }
        }
        return;
    }

    // 結果フォーカスモード: vim スタイルのナビゲーション
    if let Some(action) = app.keymap.resolve(&key, KeyContext::Overlay) {
        match action {
            Action::NavigateDown => {
                let count = app.overlays.grep_search.result_tree.visible_rows().len();
                if count == 0 {
                    return;
                }
                let selected = app.overlays.grep_search.selected;
                if app.overlays.grep_search.result_tree.is_collapsed(selected) {
                    if let Some(next) = app
                        .overlays
                        .grep_search
                        .result_tree
                        .next_sibling_index(selected)
                    {
                        app.overlays.grep_search.selected = next;
                    }
                } else if selected + 1 < count {
                    app.overlays.grep_search.selected = selected + 1;
                }
                return;
            }
            Action::NavigateUp => {
                if app.overlays.grep_search.selected > 0 {
                    app.overlays.grep_search.selected -= 1;
                }
                return;
            }
            Action::GoToTop => {
                app.overlays.grep_search.selected = 0;
                app.overlays.grep_search.scroll = 0;
                return;
            }
            Action::GoToBottom => {
                let count = app.overlays.grep_search.result_tree.visible_rows().len();
                if count > 0 {
                    app.overlays.grep_search.selected = count - 1;
                }
                return;
            }
            _ => {}
        }
    }

    match key.code {
        KeyCode::Left | KeyCode::Char('h')
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            let selected = app.overlays.grep_search.selected;
            let rows = app.overlays.grep_search.result_tree.visible_rows().to_vec();
            match rows.get(selected) {
                Some(SearchTreeRow::Dir { expanded: true, .. })
                | Some(SearchTreeRow::File { expanded: true, .. }) => {
                    app.overlays.grep_search.result_tree.collapse(selected);
                }
                Some(SearchTreeRow::Match { depth, .. }) => {
                    let d = *depth;
                    for i in (0..selected).rev() {
                        let parent_depth = match &rows[i] {
                            SearchTreeRow::Dir { depth, .. } => Some(*depth),
                            SearchTreeRow::File { depth, .. } => Some(*depth),
                            _ => None,
                        };
                        if let Some(pd) = parent_depth
                            && pd < d
                        {
                            app.overlays.grep_search.selected = i;
                            app.overlays.grep_search.result_tree.collapse(i);
                            break;
                        }
                    }
                }
                Some(SearchTreeRow::Dir {
                    expanded: false, ..
                })
                | Some(SearchTreeRow::File {
                    expanded: false, ..
                }) => {
                    let d = match &rows[selected] {
                        SearchTreeRow::Dir { depth, .. } => *depth,
                        SearchTreeRow::File { depth, .. } => *depth,
                        _ => 0,
                    };
                    if d > 0 {
                        for i in (0..selected).rev() {
                            if let SearchTreeRow::Dir { depth, .. } = &rows[i]
                                && *depth < d
                            {
                                app.overlays.grep_search.selected = i;
                                break;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        KeyCode::Right | KeyCode::Char('l')
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            app.overlays
                .grep_search
                .result_tree
                .expand(app.overlays.grep_search.selected);
        }
        // 結果フォーカスモードでのその他の文字キー: 入力に切り替えてタイプする。
        KeyCode::Char(_) => {
            app.overlays.grep_search.input_focused = true;
            if app.overlays.grep_search.query.handle_key(key) {
                app.schedule_grep_search();
            }
        }
        KeyCode::Backspace | KeyCode::Delete => {
            app.overlays.grep_search.input_focused = true;
            if app.overlays.grep_search.query.handle_key(key) {
                app.schedule_grep_search();
            }
        }
        _ => {}
    }
}

// オーバーレイ: viewer 検索

pub(in crate::event) fn handle_viewer_search_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.viewer_state.search.search_active = false;
        }
        KeyCode::Enter => {
            app.viewer_state.search.search_active = false;
            app.viewer_state.execute_search();
        }
        KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
            app.viewer_state.search.search_query.delete_to_line_start();
            app.viewer_state.execute_search();
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            clipboard_paste(app, |a| &mut a.viewer_state.search.search_query, false);
            app.viewer_state.execute_search();
        }
        _ => {
            if app.viewer_state.search.search_query.handle_key(key) {
                match key.code {
                    KeyCode::Backspace | KeyCode::Delete | KeyCode::Char(_) => {
                        app.viewer_state.execute_search();
                    }
                    _ => {}
                }
            }
        }
    }
}
