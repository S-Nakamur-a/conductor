//! パネル横断のオーバーレイ開閉ヘルパー。ファイル名検索モーダルを開く処理と、
//! 現在アクティブなオーバーレイを閉じる処理。

use crate::app::{App, WorktreeInputMode};
use crate::overlay::ActiveOverlay;
use crate::review_state::ReviewInputMode;

/// あいまいファイル名検索モーダルを開き、現在の worktree のファイル一覧で初期化する。
/// Explorer（ファイルツリー）と Viewer の両方から起動できるため、viewer が最大化されて
/// いてもファイルを切り替えられる。
pub(in crate::event) fn open_filename_search(app: &mut App) {
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
    app.viewer_state.populate_filename_search_cache();
    app.viewer_state.execute_filename_search();
}

/// フォーカス切り替えキーがどこでも機能するよう、アクティブなオーバーレイを全て閉じる。
pub(super) fn dismiss_overlays(app: &mut App) {
    app.review_state.comment_detail_active = false;
    app.review_state.input_mode = ReviewInputMode::Normal;
    app.worktree_mgr.input_mode = WorktreeInputMode::Normal;
    app.overlays.active = ActiveOverlay::None;
    app.viewer_state.filename_search.filename_search_active = false;
    app.viewer_state.search.search_active = false;
    app.review_state.search_active = false;
    app.review_state.template_picker_active = false;
    app.code_nav.references.active = false;
    // 意図的なフォーカス切り替えでは hover モーダルのスタックも閉じる。
    app.clear_hover();
}
