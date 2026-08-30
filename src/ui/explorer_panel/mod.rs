//! Explorer パネル — 中央カラムのファイルツリーブラウザ。
//!
//! 上半分に現在選択中の worktree のファイルツリーを、下半分に変更
//! （diff）ファイルの一覧を表示する。ファイル上で Enter を押すと
//! Viewer パネルで開く。
//!
//! 描画の責務ごとに分割している: [file_tree] が上半分のファイルツリーを、
//! [diff_list] が下半分の変更ファイル一覧（とそのコメントバッジ）を、
//! [comment_list] が切り替え式のレビューコメント一覧（下部ペインと
//! 全画面 C オーバーレイの両方）を、[search_box] がパネル内の
//! ファイル名検索入力欄を描画する。

use crate::app::{App, Focus};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};

mod comment_list;
mod diff_list;
mod file_tree;
mod search_box;

pub use comment_list::render_comment_list_overlay;
pub(crate) use diff_list::revidere_badge_cols;

/// 指定領域に Explorer (ファイルツリー) パネルを描画する。
pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let focused = app.focus == Focus::Explorer;

    // 上 (ファイルツリー) と下 (変更ファイル一覧) に分割する。比率は設定値で、
    // 実行中に Ctrl+Alt+↑/↓ で変えられる。マウスの当たり判定を描画と一致させる
    // ため LayoutCache の explorer_mid_y と同じ計算にすること。
    let tree_pct = app.config.layout.explorer_split_pct;
    let changed_pct = 100u16.saturating_sub(tree_pct);
    let chunks = Layout::vertical([
        Constraint::Percentage(tree_pct),
        Constraint::Percentage(changed_pct),
    ])
    .split(area);

    // イベント処理側のスクロール計算のために、実際のパネル高さを記録する。
    let tree_inner_height = chunks[0].height.saturating_sub(2) as usize;
    // 変更ファイル一覧は先頭行をエラーバナーに使うが、これは display_list の
    // 要素ではない。スクロールのページ幅とマウスの行→インデックス変換の両方が
    // この行数を知る必要があるので、どのビューが表示中かを唯一知っている
    // ここから公開する。
    let shows_error_banner = app.explorer.bottom_view
        == crate::viewer::ExplorerBottomView::DiffList
        && app.diff_state.error.is_some();
    let banner_rows = diff_list::diff_list_banner_rows(shows_error_banner);
    let diff_inner_height =
        (chunks[1].height.saturating_sub(2) as usize).saturating_sub(banner_rows);
    app.explorer.tree_height = tree_inner_height.max(1);
    app.explorer.diff_list_height = diff_inner_height.max(1);
    app.explorer.diff_banner_rows = banner_rows;

    file_tree::render_file_tree(frame, chunks[0], app, focused);
    match app.explorer.bottom_view {
        crate::viewer::ExplorerBottomView::Comments => {
            comment_list::render_comment_list(frame, chunks[1], app, focused);
        }
        crate::viewer::ExplorerBottomView::DiffList => {
            diff_list::render_diff_list(frame, chunks[1], app, focused);
        }
    }

    // 検索入力のオーバーレイを出す (全体オーバーレイに覆われている間はカーソル配置をしない)。
    let overlay_active = app.is_any_overlay_active();
    if app.viewer.search.search_active {
        search_box::render_search_box(
            frame,
            area,
            &app.viewer.search.search_query,
            &app.theme,
            overlay_active,
        );
    }

    // ファイル名のあいまい検索モーダルは最上位で描画している
    // (layout::render_ui を参照)。このパネルが幅ゼロまで畳まれていても
    // (Viewer 最大化中など) 見えたままにするため。
}
