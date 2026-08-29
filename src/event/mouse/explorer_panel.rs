//! Explorerカラム（ファイルツリー / 差分リスト / コメントリスト）のクリック処理。

use crate::app::{App, Focus};

use super::super::explorer::navigate_to_comment_with_focus;
use super::{ClickGeometry, register_double_click_on};

/// /conductor:address-conductor-comment 経由で未対応コメントを全てClaudeに送る（IDなし＝一括モード）。
fn ask_claude_all_comments(app: &mut App) {
    let prompt = "/conductor:address-conductor-comment\n".to_string();
    if let Some(idx) = app.terminal.claude.active_session {
        if app.terminal.pty_manager.is_waiting_for_input(idx) {
            let _ = app
                .terminal
                .pty_manager
                .write_chunked_to_session(idx, &prompt);
        } else {
            app.terminal.deferred_prompts.insert(idx, prompt);
        }
        app.set_focus(Focus::TerminalClaude);
        app.set_status(
            "Sent all comments to Claude".to_string(),
            crate::app::StatusLevel::Info,
        );
    } else {
        app.set_status(
            "No active Claude Code session".to_string(),
            crate::app::StatusLevel::Warning,
        );
    }
}

/// 画面上のセルをファイルツリーの表示リストインデックス（ViewerState::visible_indices()
/// へのインデックス）に解決する。セルがツリー行の上にない場合（列違い、Explorerの下半分、
/// 先頭行より上）は None を返す。実際の表示行数との照合はしない — 従来のクリック処理と
/// 同様、それは呼び出し側が visible.get(idx) で行う。
///
/// クリックハンドラとホバートラッカー（event/mouse/mod.rs の Moved）の両方から使われる。
/// これにより、ハイライトされた行とクリックで開かれる行が食い違うことは構造的にあり得ない。
pub(super) fn explorer_tree_row_at(
    geom: &ClickGeometry,
    scroll: usize,
    col: u16,
    row: u16,
) -> Option<usize> {
    if col < geom.left_end || col >= geom.explorer_end {
        return None;
    }
    // explorer_mid_y は「変更されたファイル」パネルの上枠なので、ファイルツリー自体の
    // 下枠はその1行上にある。両方とも弾く必要がある: ツリーは height - 2 行のコンテンツを
    // 描画するので、枠を通してしまうと scroll + inner_height — 実際に描画された最後の行の
    // さらに1つ先 — が返ってしまう。
    //
    // 通常この問題は隠れている。divider_at がリサイズ用にこの2行を先に取ってしまうからだが、
    // divider_draggable はパネルが最大化されている間 false を返す。そのためExplorerを最大化
    // した状態で水平線をクリックするとここに落ちてきて、画面に出ていないファイルが開いてしまう
    // バグがあった。兄弟にあたる diff_list_row_at は元々自分の下枠を除外していたので、
    // この非対称性がバグの正体だった。
    if row >= geom.explorer_mid_y.saturating_sub(1) {
        return None;
    }
    let inner_y = geom.main_area.y + 1; // 枠の内側
    if row < inner_y {
        return None;
    }
    let offset = (row - inner_y) as usize;
    Some(scroll + offset)
}

/// 画面上のセルを「変更されたファイル」（差分リスト）の表示リストインデックス
/// （DiffState::display_list へのインデックス）に解決する。セルが差分リストの行の上に
/// ない場合（列違い、Explorerの上半分、先頭行より上、パネルの下枠上またはそれより下 —
/// 下枠には「Ask Claude All」ボタンがあり、リスト行ではない）は None を返す。
/// [explorer_tree_row_at] と同様、実際の行数との照合は呼び出し側が
/// display_list.get(idx) で行う。
///
/// クリックハンドラとホバートラッカーの両方から使われるため、「光っている行」と
/// 「開かれる行」が食い違うことは構造的にあり得ない。
///
/// banner_rows はパネルがリストの上に描くエラーバナーの高さ。これらの行は画面上には
/// あるが display_list には含まれないため、各エントリはインデックスが示す位置より
/// その分だけ下にずれる。このオフセットは両方の呼び出し側が必要とするため、
/// 呼び出し側ではなくここに置いている。
pub(super) fn diff_list_row_at(
    geom: &ClickGeometry,
    scroll: usize,
    banner_rows: usize,
    col: u16,
    row: u16,
) -> Option<usize> {
    if col < geom.left_end || col >= geom.explorer_end {
        return None;
    }
    if row < geom.explorer_mid_y {
        return None;
    }
    let bottom_border_y = geom.main_area.y + geom.main_area.height.saturating_sub(1);
    if row >= bottom_border_y {
        return None;
    }
    let inner_y = geom.explorer_mid_y + 1; // inside border
    if row < inner_y {
        return None;
    }
    let offset = (row - inner_y) as usize;
    // バナー自体の上のセルはどのエントリの上でもない: これがないと、メッセージを
    // クリックしたときにたまたま一番上にスクロールされていた項目が開いてしまう。
    Some(scroll + offset.checked_sub(banner_rows)?)
}

/// Explorerカラム（ファイルツリー / 差分リスト / コメントリスト）内の左クリックを処理する。
pub(super) fn handle_explorer_column_click(
    app: &mut App,
    col: u16,
    row: u16,
    geom: &ClickGeometry,
) {
    let main_area = geom.main_area;
    let explorer_mid_y = geom.explorer_mid_y;
    let explorer_end = geom.explorer_end;

    app.set_focus(Focus::Explorer);

    // クリックが上半分（ファイルツリー）か下半分（差分/コメントリスト）かを判定する。
    if row >= explorer_mid_y {
        app.viewer_state.explorer.explorer_focus_on_diff_list = true;

        // 下枠の「✨ Ask Claude All」ボタンへのクリックかを確認する。
        let bottom_border_y = main_area.y + main_area.height.saturating_sub(1);
        if row == bottom_border_y
            && app.viewer_state.explorer.explorer_bottom_view
                == crate::viewer::ExplorerBottomView::Comments
        {
            // 「 ✨ Ask Claude All 」は右揃えで、右端から約19文字。
            let ask_label_w = 19_u16;
            let ask_start_col = explorer_end.saturating_sub(ask_label_w + 1);
            if col >= ask_start_col && col < explorer_end {
                ask_claude_all_comments(app);
                return;
            }
        }

        let inner_y = explorer_mid_y + 1; // 枠の内側
        if row >= inner_y {
            let click_offset = (row - inner_y) as usize;

            if app.viewer_state.explorer.explorer_bottom_view
                == crate::viewer::ExplorerBottomView::Comments
            {
                // コメントリストが表示されている場合 — コメント選択を処理する。
                let idx = app.viewer_state.explorer.comment_list_scroll + click_offset;
                let row_count = app.review_state.comment_list_rows.len();
                if idx < row_count {
                    app.viewer_state.explorer.comment_list_selected = idx;

                    // ダブルクリック検出。
                    let is_double = register_double_click_on(
                        &mut app.viewer_state.click.last_comment_click_time,
                        &mut app.viewer_state.click.last_comment_click_idx,
                        idx,
                        std::time::Instant::now(),
                    );

                    // コメントのファイル位置へ移動する。
                    if let Some(comment_idx) = app.review_state.selected_comment_idx(idx) {
                        // シングルクリック: 位置へジャンプし、フォーカスはコメント側に残す。
                        // ダブルクリック: ジャンプしてViewerにフォーカスを移す。
                        navigate_to_comment_with_focus(app, comment_idx, is_double);
                    }
                }
            } else if app.viewer_state.explorer.explorer_bottom_view
                == crate::viewer::ExplorerBottomView::DiffList
            {
                // 差分リストが表示されている場合 — 差分選択を処理する。
                let scroll = app.viewer_state.explorer.diff_list_scroll;
                let banner = app.viewer_state.explorer.explorer_diff_banner_rows;
                if let Some(idx) = diff_list_row_at(geom, scroll, banner, col, row)
                    && idx < app.diff_state.display_list.len()
                {
                    app.viewer_state.explorer.diff_list_selected = idx;
                    // シングルクリック: SUMMARY擬似ファイルなら変更サマリーを開く。
                    if matches!(
                        app.diff_state.display_list.get(idx),
                        Some(crate::diff_state::DiffListEntry::Summary {})
                    ) {
                        app.viewer_state.enter_summary_view();
                        app.set_focus(Focus::Viewer);
                    }
                    // シングルクリック: ヘッダーの開閉、またはViewerでファイルを開く。
                    else if app.diff_state.toggle_section(idx) {
                        // セクションヘッダーを開閉した。
                        let new_count = app.diff_state.display_list.len();
                        if new_count > 0
                            && app.viewer_state.explorer.diff_list_selected >= new_count
                        {
                            app.viewer_state.explorer.diff_list_selected = new_count - 1;
                        }
                    } else if app.diff_state.resolve_file(idx).is_some() {
                        // diff_list_selected は既にこの行を指している。共通のオープン処理は
                        // コメントがあれば最初のコメントへ着地する。
                        app.open_diff_file_at_selected();
                        app.set_focus(Focus::Viewer);
                    }
                }
            }
        }
    } else {
        app.viewer_state.explorer.explorer_focus_on_diff_list = false;
        // クリックされたファイルツリーの項目を選択する。
        let scroll = app.viewer_state.tree.tree_scroll;
        if let Some(idx) = explorer_tree_row_at(geom, scroll, col, row) {
            let visible = app.viewer_state.visible_indices();
            if let Some(&tree_idx) = visible.get(idx) {
                app.viewer_state.tree.tree_selected = tree_idx;
                // シングルクリックでViewerにファイルを開く（ディレクトリなら開閉する）。
                if let Some(entry) = app.viewer_state.tree.file_tree.get(tree_idx).cloned() {
                    if entry.is_dir {
                        // 展開する前に子要素を遅延読み込みする。
                        if !entry.is_expanded {
                            app.viewer_state.ensure_children_loaded(tree_idx);
                        }
                        app.viewer_state.toggle_dir(tree_idx);
                    } else {
                        // ダブルクリック検出。
                        let is_double = register_double_click_on(
                            &mut app.viewer_state.click.last_tree_click_time,
                            &mut app.viewer_state.click.last_tree_click_idx,
                            tree_idx,
                            std::time::Instant::now(),
                        );

                        let tab_width = app.config.viewer.tab_width;
                        // シングルクリックは preview（次にどれかを開くと閉じる）、
                        // ダブルクリックは永続。開いたまま溜まるのを防ぐ。
                        if is_double {
                            app.viewer_state.open_file(&entry.path, tab_width);
                        } else {
                            app.viewer_state.open_file_preview(&entry.path, tab_width);
                        }
                        app.rehighlight_viewer();
                        app.review_state.build_file_comment_cache(&entry.path);
                        // シングルクリック: フォーカスをExplorerに残す。
                        // ダブルクリック: フォーカスをViewerに移す。
                        if is_double {
                            app.set_focus(Focus::Viewer);
                        }
                    }
                }
            }
        }
    }
}
