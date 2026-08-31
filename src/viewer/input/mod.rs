//! Viewer パネルのキー処理。
//!
//! エントリポイント（handle_viewer_key）と、プレーンファイル・統合 diff・
//! summary 疑似ファイルのナビゲーション振り分け。関連する処理はサブモジュール
//! に分かれている: diff_nav（diff ビュー向けの純粋な変更ブロック/コメント
//! ナビゲーションヘルパー）、inline_reply（インラインコメントスレッドの
//! 開閉と返信入力）、code_nav（g プレフィックスで発火する定義へ移動・
//! 実装へ移動・参照検索）。

pub(crate) mod code_nav;
mod diff_nav;
mod inline_reply;

use crossterm::event::{KeyCode, KeyEvent};

use crate::app::App;
use crate::keymap::{Action, KeyContext};

use crate::viewer::comment_actions::open_viewer_comment_detail;

use code_nav::{
    handle_find_references, handle_go_to_definition, handle_go_to_implementation,
    handle_show_hover_info,
};
use diff_nav::{next_change_block, next_comment_line, prev_change_block, prev_comment_line};
use inline_reply::{handle_inline_reply_input, start_inline_reply, toggle_inline_thread};

/// 素のファイルビューと統合 diff で共有し、g の意味を揃える。diff では呼び出し側が先に
/// content.file_scroll を diff カーソルへ同期させる — ヒントも同じ位置から作る。
fn enter_g_prefix_mode(app: &mut App) {
    app.viewer.pending_g_key = true;
    let hints = app.build_symbol_hints(50);
    app.code_nav.symbol_hint.active = !hints.is_empty();
    app.code_nav.symbol_hint.hints = hints;
    app.code_nav.symbol_hint.input.clear();
}

/// タブの切り替え/クローズ。素のファイル表示・diff 表示・レンダリング済み
/// markdown のどれでも同じ意味なので、3 つの分岐で共有する。
fn handle_tab_action(app: &mut App, action: Option<Action>) -> bool {
    match action {
        Some(Action::NextViewerTab) => app.next_viewer_tab(),
        Some(Action::PrevViewerTab) => app.prev_viewer_tab(),
        Some(Action::CloseViewerTab) => app.close_viewer_tab(None),
        _ => return false,
    }
    true
}

/// Viewer パネルがフォーカスされているときのキーを処理する。
pub(crate) fn handle_viewer_key(app: &mut App, key: KeyEvent) -> Option<KeyEvent> {
    if app.viewer.inline.reply_line.is_some() {
        handle_inline_reply_input(app, key);
        return None;
    }

    if app.viewer.is_summary() {
        return handle_viewer_summary_mode_key(app, key);
    }

    // レンダリング済み markdown には行番号が無いのでビュー全体のナビゲーションのみ。ヒントを
    // 行番号で解決する g プレフィックスより先にチェックする。
    if app.viewer.is_showing_rendered_markdown() {
        return handle_viewer_markdown_mode_key(app, key);
    }

    // gd/gi/gr/gg はプレーンファイル表示でも diff 表示でも同じなので、diff モードの
    // 振り分けより先にチェックする。
    if app.viewer.pending_g_key {
        app.viewer.pending_g_key = false;
        match key.code {
            KeyCode::Char('d') => {
                app.code_nav.symbol_hint = Default::default();
                handle_go_to_definition(app);
                return None;
            }
            KeyCode::Char('i') => {
                app.code_nav.symbol_hint = Default::default();
                handle_go_to_implementation(app);
                return None;
            }
            KeyCode::Char('r') => {
                app.code_nav.symbol_hint = Default::default();
                handle_find_references(app);
                return None;
            }
            // gK / gh — hover 情報。ヒントラベルは常に小文字なので、これらの予約キーと衝突しない。
            KeyCode::Char('K') | KeyCode::Char('h') => {
                app.code_nav.symbol_hint = Default::default();
                handle_show_hover_info(app);
                return None;
            }
            KeyCode::Char('g') => {
                app.code_nav.symbol_hint = Default::default();
                if app.viewer.diff_view.diff_mode {
                    app.viewer.diff_view.diff_view_scroll = 0;
                } else {
                    app.viewer.content.file_scroll = 0;
                }
                return None;
            }
            KeyCode::Esc => {
                app.code_nav.symbol_hint = Default::default();
                return None;
            }
            KeyCode::Char(c) if c.is_ascii_lowercase() => {
                app.code_nav.symbol_hint.input.push(c);
                return None;
            }
            _ => {
                app.code_nav.symbol_hint = Default::default();
            }
        }
    }

    // プレーンファイル表示専用。diff 表示の折りたたみは ExpandableContext という別の仕組み
    // なので、同じキーに 2 つの意味を持たせない。
    if app.viewer.pending_z_key {
        app.viewer.pending_z_key = false;
        if !app.viewer.diff_view.diff_mode {
            return handle_fold_key(app, key);
        }
    }

    if app.viewer.diff_view.diff_mode {
        return handle_viewer_diff_mode_key(app, key);
    }

    let total = app.viewer.content.file_content.len();
    let action = app.keymap.resolve(&key, KeyContext::Viewer);

    if let Some(Action::ExitToExplorer) = action {
        if app.viewer.selection != crate::viewer::LineSelection::None {
            app.viewer.clear_selection();
        } else {
            app.set_focus(crate::app::Focus::Explorer);
        }
        return None;
    }

    // 空バッファのガードより前で処理する。ファイルが開かれていなくても動き、ジャンプ後も
    // viewer が最大化されたままになる。
    if let Some(Action::SearchFilename) = action {
        crate::event::open_filename_search(app);
        return None;
    }

    // 空バッファのガードより前へ。ファイルが無いときは「何も起きない」ではなくヒントを出す。
    if let Some(Action::OpenInEditor) = action {
        app.open_in_editor();
        return None;
    }

    // タブ操作も空バッファのガードより前へ。読めなかったファイルのタブが閉じられなくなる。
    if handle_tab_action(app, action) {
        return None;
    }

    if total == 0 {
        return None;
    }

    match action {
        // 移動はすべて可視行を歩く（畳んだ中にカーソルが入らない）。
        Some(Action::NavigateDown) => app.viewer.move_cursor_lines(1),
        Some(Action::NavigateUp) => app.viewer.move_cursor_lines(-1),
        Some(Action::ScrollHalfPageDown) => app.viewer.move_cursor_lines(15),
        Some(Action::ScrollHalfPageUp) => app.viewer.move_cursor_lines(-15),
        Some(Action::GoToTop) => enter_g_prefix_mode(app),
        Some(Action::GoToBottom) => app.viewer.goto_last_visible_line(),
        Some(Action::FoldPrefix) => app.viewer.pending_z_key = true,
        Some(Action::SearchInFile) => {
            app.viewer.search.search_active = true;
            app.viewer.search.search_query.clear();
        }
        Some(Action::NextSearchMatch) => {
            app.viewer.next_search_match();
        }
        Some(Action::PrevSearchMatch) => {
            app.viewer.prev_search_match();
        }
        Some(Action::ScrollLeft) => {
            app.viewer.content.h_scroll = app.viewer.content.h_scroll.saturating_sub(4);
        }
        Some(Action::ScrollRight) => {
            app.viewer.scroll_right(4);
        }
        Some(Action::ScrollHome) => {
            app.viewer.content.h_scroll = 0;
        }
        Some(Action::ToggleInlineThread) => {
            toggle_inline_thread(app);
        }
        Some(Action::InlineReply) => {
            start_inline_reply(app);
        }
        Some(Action::ViewCommentDetail) => {
            open_viewer_comment_detail(app);
        }
        Some(Action::AddComment) => {
            app.cmd_add_review_comment();
        }
        Some(Action::JumpBack) => {
            app.jump_back();
        }
        Some(Action::JumpForward) => {
            app.jump_forward();
        }
        Some(Action::ShowHoverInfo) => {
            handle_show_hover_info(app);
        }
        Some(Action::ToggleMarkdownRender) => {
            app.cmd_toggle_markdown_render();
        }
        _ => {}
    }
    None
}

/// 対象はカーソル行 (ビューポート最上行)。見出しでなければそれを含む最も内側のブロックが
/// 動く (vim の za/zc と同じ)。zm/zr だけは行ではなく深さを対象にする。
fn handle_fold_key(app: &mut App, key: KeyEvent) -> Option<KeyEvent> {
    match key.code {
        KeyCode::Char('a') => {
            app.viewer.fold_toggle_cursor();
        }
        KeyCode::Char('c') => {
            app.viewer.fold_close_cursor();
        }
        KeyCode::Char('o') => {
            app.viewer.fold_open_cursor();
        }
        KeyCode::Char('m') => app.cmd_fold_one_level(),
        KeyCode::Char('r') => app.cmd_unfold_one_level(),
        KeyCode::Char('R') => app.cmd_unfold_all(),
        KeyCode::Char('M') => app.cmd_fold_all(),
        _ => {}
    }
    None
}

/// レンダリング済み markdown ビューのナビゲーション。
///
/// 全ディスパッチではなく許可リストにしてある。ここに無い操作 (コメント作成、インライン
/// スレッド、ファイル内検索、hover/シンボルジャンプ、水平スクロール) はすべてソース行
/// 単位でアドレスするもので、レンダリングされた本文には対応する行番号が無い。
pub(super) fn handle_viewer_markdown_mode_key(app: &mut App, key: KeyEvent) -> Option<KeyEvent> {
    let total = app.viewer.md_total_lines;
    let action = app.keymap.resolve(&key, KeyContext::Viewer);

    if handle_tab_action(app, action) {
        return None;
    }

    match action {
        Some(Action::ToggleMarkdownRender) => app.cmd_toggle_markdown_render(),
        Some(Action::ExitToExplorer) => app.set_focus(crate::app::Focus::Explorer),
        Some(Action::SearchFilename) => crate::event::open_filename_search(app),
        // ファイル単位であって行単位ではないので、レンダリング表示からでも意味がある。
        Some(Action::OpenInEditor) => app.open_in_editor(),
        Some(Action::NavigateDown) if app.viewer.md_scroll + 1 < total => {
            app.viewer.md_scroll += 1;
        }
        Some(Action::NavigateUp) => {
            app.viewer.md_scroll = app.viewer.md_scroll.saturating_sub(1);
        }
        Some(Action::ScrollHalfPageDown) => {
            app.viewer.md_scroll = (app.viewer.md_scroll + 15).min(total.saturating_sub(1));
        }
        Some(Action::ScrollHalfPageUp) => {
            app.viewer.md_scroll = app.viewer.md_scroll.saturating_sub(15);
        }
        Some(Action::GoToTop) => app.viewer.md_scroll = 0,
        Some(Action::GoToBottom) => {
            app.viewer.md_scroll = total.saturating_sub(1);
        }
        _ => {}
    }
    None
}

/// Viewer パネルの統合 diff モードでのキー処理。
/// summary 疑似ファイルビューをナビゲートする: スクロール、両端へのジャンプ、
/// または Explorer へ抜ける。diff モードのキーコンテキストを再利用するので、
/// j/k/d/u/g/G/Esc は他の場所と同じように振る舞う。
pub(super) fn handle_viewer_summary_mode_key(app: &mut App, key: KeyEvent) -> Option<KeyEvent> {
    let total = app.viewer.summary_total_lines;
    let action = app.keymap.resolve(&key, KeyContext::ViewerDiffMode);

    match action {
        Some(Action::ExitToExplorer) => {
            app.viewer.exit_diff_mode(); // show_summary もクリアする
            app.set_focus(crate::app::Focus::Explorer);
        }
        Some(Action::SearchFilename) => crate::event::open_filename_search(app),
        Some(Action::NavigateDown) if app.viewer.summary_scroll + 1 < total => {
            app.viewer.summary_scroll += 1;
        }
        Some(Action::NavigateUp) => {
            app.viewer.summary_scroll = app.viewer.summary_scroll.saturating_sub(1);
        }
        Some(Action::ScrollHalfPageDown) => {
            app.viewer.summary_scroll =
                (app.viewer.summary_scroll + 15).min(total.saturating_sub(1));
        }
        Some(Action::ScrollHalfPageUp) => {
            app.viewer.summary_scroll = app.viewer.summary_scroll.saturating_sub(15);
        }
        Some(Action::GoToTop) => app.viewer.summary_scroll = 0,
        Some(Action::GoToBottom) => {
            app.viewer.summary_scroll = total.saturating_sub(1);
        }
        _ => {}
    }
    None
}

pub(super) fn handle_viewer_diff_mode_key(app: &mut App, key: KeyEvent) -> Option<KeyEvent> {
    let total = app.viewer.diff_view.diff_view_lines.len();
    let action = app.keymap.resolve(&key, KeyContext::ViewerDiffMode);

    if let Some(Action::ExitToExplorer) = action {
        if app.viewer.selection != crate::viewer::LineSelection::None {
            app.viewer.clear_selection();
        } else {
            app.viewer.exit_diff_mode();
            app.set_focus(crate::app::Focus::Explorer);
        }
        return None;
    }

    if let Some(Action::SearchFilename) = action {
        crate::event::open_filename_search(app);
        return None;
    }

    if handle_tab_action(app, action) {
        return None;
    }

    // 次/前の変更ファイルへジャンプ（GitHub 風の「次のファイル」）。
    if let Some(Action::NextChangedFile) = action {
        app.jump_to_changed_file(true);
        return None;
    }
    if let Some(Action::PrevChangedFile) = action {
        app.jump_to_changed_file(false);
        return None;
    }

    // 空バッファのガードより前に処理するので、空の diff でも動作する。
    if let Some(Action::OpenInEditor) = action {
        app.open_in_editor();
        return None;
    }

    if total == 0 {
        return None;
    }

    match action {
        Some(Action::NavigateDown) if app.viewer.diff_view.diff_view_scroll + 1 < total => {
            app.viewer.diff_view.diff_view_scroll += 1;
        }
        Some(Action::NavigateUp) => {
            app.viewer.diff_view.diff_view_scroll =
                app.viewer.diff_view.diff_view_scroll.saturating_sub(1);
        }
        Some(Action::ScrollHalfPageDown) => {
            app.viewer.diff_view.diff_view_scroll =
                (app.viewer.diff_view.diff_view_scroll + 15).min(total.saturating_sub(1));
        }
        Some(Action::ScrollHalfPageUp) => {
            app.viewer.diff_view.diff_view_scroll =
                app.viewer.diff_view.diff_view_scroll.saturating_sub(15);
        }
        Some(Action::GoToTop) => {
            // 'g' はプレーンファイルビューと同じシンボルヒントプレフィックス (gd/gi/gr/gg または
            // ヒントラベル) にマッチする。シンボル検索とヒント構築はこのフィールドを読むので、
            // まず content.file_scroll を diff カーソル下の行に同期させる。
            app.viewer.sync_file_scroll_to_diff_scroll();
            enter_g_prefix_mode(app);
        }
        Some(Action::GoToBottom) => {
            app.viewer.diff_view.diff_view_scroll = total.saturating_sub(1);
        }
        Some(Action::SearchInFile) => {
            app.viewer.sync_file_scroll_to_diff_scroll();
            app.viewer.search.search_active = true;
            app.viewer.search.search_query.clear();
        }
        Some(Action::NextSearchMatch) => {
            app.viewer.next_search_match();
        }
        Some(Action::PrevSearchMatch) => {
            app.viewer.prev_search_match();
        }
        Some(Action::NextHunk) => {
            let lines = &app.viewer.diff_view.diff_view_lines;
            if let Some(idx) = next_change_block(lines, app.viewer.diff_view.diff_view_scroll) {
                app.viewer.diff_view.diff_view_scroll = idx;
            }
        }
        Some(Action::PrevHunk) => {
            let lines = &app.viewer.diff_view.diff_view_lines;
            if let Some(idx) = prev_change_block(lines, app.viewer.diff_view.diff_view_scroll) {
                app.viewer.diff_view.diff_view_scroll = idx;
            }
        }
        Some(Action::NextComment) => {
            let idx = next_comment_line(
                &app.viewer.diff_view.diff_view_lines,
                &app.review_state.file_comments,
                app.viewer.diff_view.diff_view_scroll,
            );
            if let Some(idx) = idx {
                app.viewer.diff_view.diff_view_scroll = idx;
            }
        }
        Some(Action::PrevComment) => {
            let idx = prev_comment_line(
                &app.viewer.diff_view.diff_view_lines,
                &app.review_state.file_comments,
                app.viewer.diff_view.diff_view_scroll,
            );
            if let Some(idx) = idx {
                app.viewer.diff_view.diff_view_scroll = idx;
            }
        }
        Some(Action::ScrollLeft) => {
            app.viewer.content.h_scroll = app.viewer.content.h_scroll.saturating_sub(4);
        }
        Some(Action::ScrollRight) => {
            app.viewer.scroll_right(4);
        }
        Some(Action::ScrollHome) => {
            app.viewer.content.h_scroll = 0;
        }
        Some(Action::ToggleInlineThread) => {
            toggle_inline_thread(app);
        }
        Some(Action::InlineReply) => {
            start_inline_reply(app);
        }
        Some(Action::ViewCommentDetail) => {
            open_viewer_comment_detail(app);
        }
        Some(Action::AddComment) => {
            app.cmd_add_review_comment();
        }
        Some(Action::ExpandContext) => {
            if let Some(idx) = app.viewer.find_visible_expandable(50) {
                app.viewer.expand_context_at(idx, false);
            }
        }
        Some(Action::ExpandAllContext) => {
            if let Some(idx) = app.viewer.find_visible_expandable(50) {
                app.viewer.expand_context_at(idx, true);
            }
        }
        Some(Action::ToggleViewed) => {
            if let Some(path) = app.viewer.content.current_file.clone() {
                app.toggle_path_viewed(&path);
            }
        }
        Some(Action::ShowHoverInfo) => {
            app.viewer.sync_file_scroll_to_diff_scroll();
            handle_show_hover_info(app);
        }
        _ => {}
    }
    None
}
