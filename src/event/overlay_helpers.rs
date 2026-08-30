//! パネル横断のオーバーレイ開閉ヘルパー。ファイル名検索モーダルを開く処理。

use crate::app::App;

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
