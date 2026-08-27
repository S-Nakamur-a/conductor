//! マウスのヒットテストジオメトリ、ダブルクリック判定、左マージンの
//! クリック分類に対する単体テスト。

use super::explorer_panel::{diff_list_row_at, explorer_tree_row_at};
use super::viewer_panel::{
    MarginClickAction, MarginZone, classify_margin_click, thread_anchor_line,
};
use super::{
    ClickGeometry, Column, in_fold_zone, register_double_click, register_double_click_on,
    terminal_tab_row_at,
};
use std::time::{Duration, Instant};

/// 指定したカラム境界でClickGeometryを構築する。幅/高さは、[<=>] 展開ボタン
/// （各カラム境界の手前5列、幅7以上が必要）がテスト可能になるよう設定する。
fn geom(left_end: u16, explorer_end: u16, viewer_end: u16) -> ClickGeometry {
    ClickGeometry {
        main_area: ratatui::layout::Rect::new(0, 1, viewer_end + 20, 40),
        left_w: left_end,
        explorer_w: explorer_end - left_end,
        viewer_w: viewer_end - explorer_end,
        left_end,
        explorer_end,
        viewer_end,
        explorer_mid_y: 20,
        terminal_claude_y: 1,
        terminal_split_y: 33,
    }
}

#[test]
fn gutter_and_badge_clicks_always_start_a_comment() {
    // 重なりの修正の核心部分: 既存のコメント範囲に含まれる行（has_comment = true）
    // であっても、行番号ガターと「+」バッジ列からは常に新しいコメントを開始
    // しなければならず、スレッドのフォーカスに飲み込まれてはならない。
    // このアフォーダンスはどの行でも同じ。
    for zone in [MarginZone::NumberGutter, MarginZone::Badge] {
        for has_comment in [false, true] {
            assert_eq!(
                classify_margin_click(zone, has_comment, false, false),
                MarginClickAction::StartComment { extend: false }
            );
            assert_eq!(
                classify_margin_click(zone, has_comment, false, true),
                MarginClickAction::StartComment { extend: true }
            );
        }
    }
    // テストが実行可能な行であっても、行番号ガターはコメントを開始する。
    assert_eq!(
        classify_margin_click(MarginZone::NumberGutter, false, true, false),
        MarginClickAction::StartComment { extend: false }
    );
}

#[test]
fn marker_click_focuses_existing_thread() {
    // 一番左の💬/│マーカー列だけがスレッドのフォーカスを持つ唯一の場所。
    assert_eq!(
        classify_margin_click(MarginZone::Marker, true, false, false),
        MarginClickAction::ToggleThread
    );
    // コメント済みのテスト行でもコメントが優先される（▶はバッジ列にあり、
    // ここにはない）。
    assert_eq!(
        classify_margin_click(MarginZone::Marker, true, true, false),
        MarginClickAction::ToggleThread
    );
    // 空のマーカーセルはコメント開始にフォールバックする。
    assert_eq!(
        classify_margin_click(MarginZone::Marker, false, false, false),
        MarginClickAction::StartComment { extend: false }
    );
}

#[test]
fn badge_click_on_test_line_runs_the_test() {
    assert_eq!(
        classify_margin_click(MarginZone::Badge, false, true, false),
        MarginClickAction::RunTest
    );
    // コメント済みのテスト行でも: ▶はバッジ列に描画され続けるので、そこへの
    // クリックはやはりテストを実行する（スレッドのフォーカスはマーカーの
    // 仕事）。
    assert_eq!(
        classify_margin_click(MarginZone::Badge, true, true, false),
        MarginClickAction::RunTest
    );
}

#[test]
fn thread_anchor_redirects_mid_range_lines_to_nearest_end_line() {
    use crate::review_store::{Author, CommentKind, CommentStatus, ReviewComment};
    let range = |id: &str, start: u32, end: u32| ReviewComment {
        id: id.to_string(),
        worktree: "wt".to_string(),
        file_path: "src/main.rs".to_string(),
        line_start: start,
        line_end: Some(end),
        kind: CommentKind::Suggest,
        body: "body".to_string(),
        status: CommentStatus::Pending,
        author: Author::User,
        branch: None,
        created_at: String::new(),
    };
    // 入れ子になった範囲 L10-L20 と L11-L19: 範囲の途中の│クリックは、両方の
    // 表示でスレッドが描画される、最も近い終了行（💬）に着地する。
    let both = [range("outer", 10, 20), range("inner", 11, 19)];
    assert_eq!(thread_anchor_line(&both, 15), 19);
    // 外側の範囲にしか含まれない行は、その終了行に固定される。
    let outer_only = [range("outer", 10, 20)];
    assert_eq!(thread_anchor_line(&outer_only, 10), 20);
    // 終了行自体は自分自身に固定される。
    assert_eq!(thread_anchor_line(&both, 19), 19);
    assert_eq!(thread_anchor_line(&outer_only, 20), 20);
}

#[test]
fn divider_at_hits_both_cells_of_a_vertical_boundary() {
    use crate::app::Divider;
    // main_areaはyが[1, 41)の範囲。縦の境界は{edge-1, edge}の2セル分のゾーン。
    let g = geom(0, 24, 62);
    assert_eq!(g.divider_at(23, 10), Some(Divider::ExplorerViewer));
    assert_eq!(g.divider_at(24, 10), Some(Divider::ExplorerViewer));
    assert_eq!(g.divider_at(61, 10), Some(Divider::ViewerTerminal));
    assert_eq!(g.divider_at(62, 10), Some(Divider::ViewerTerminal));
    // ゾーンの両側1セルはヒットしない。
    assert_eq!(g.divider_at(22, 10), None);
    assert_eq!(g.divider_at(25, 10), None);
}

#[test]
fn divider_at_hits_horizontal_splits_within_their_column() {
    use crate::app::Divider;
    let g = geom(0, 24, 62); // explorer_mid_y=20, terminal_split_y=33
    // Explorerの分割線: Explorer列 [0, 24) の内側でのみ。
    assert_eq!(g.divider_at(10, 19), Some(Divider::ExplorerSplit));
    assert_eq!(g.divider_at(10, 20), Some(Divider::ExplorerSplit));
    assert_eq!(g.divider_at(10, 18), None);
    // Explorerの分割線はViewer/Terminal列には広がらない。
    assert_eq!(g.divider_at(40, 20), None);
    // Terminalの分割線: Terminal列 [62, 右端) の内側でのみ。
    assert_eq!(g.divider_at(70, 32), Some(Divider::TerminalSplit));
    assert_eq!(g.divider_at(70, 33), Some(Divider::TerminalSplit));
    assert_eq!(g.divider_at(40, 33), None);
}

#[test]
fn divider_at_vertical_boundary_wins_at_a_corner() {
    use crate::app::Divider;
    // (explorer_end-1, explorer_mid_y) は縦の境界セルであると同時にExplorerの
    // 分割線の行でもある — 縦の境界が優先されなければならない。
    let g = geom(0, 24, 62);
    assert_eq!(g.divider_at(23, 20), Some(Divider::ExplorerViewer));
}

#[test]
fn divider_at_misses_open_panel_area() {
    let g = geom(0, 24, 62);
    assert_eq!(g.divider_at(10, 10), None);
    assert_eq!(g.divider_at(70, 10), None);
}

#[test]
fn column_at_maps_columns_by_boundary() {
    let g = geom(20, 50, 90);
    assert_eq!(g.column_at(0), Column::Worktree);
    assert_eq!(g.column_at(19), Column::Worktree);
    assert_eq!(g.column_at(20), Column::Explorer);
    assert_eq!(g.column_at(49), Column::Explorer);
    assert_eq!(g.column_at(50), Column::Viewer);
    assert_eq!(g.column_at(89), Column::Viewer);
    assert_eq!(g.column_at(90), Column::Terminal);
    assert_eq!(g.column_at(200), Column::Terminal);
}

/// ホイールでタブを送れるのは、ターミナル列の 2 本のタブ帯の行だけ。
/// 本文の行まで拾うと、スクロールバックを遡れなくなる。
#[test]
fn terminal_tab_rows_are_only_the_two_strip_rows() {
    let g = geom(20, 50, 90);
    assert_eq!(terminal_tab_row_at(95, g.terminal_claude_y, &g), Some(true));
    assert_eq!(terminal_tab_row_at(95, g.terminal_split_y, &g), Some(false));
    assert_eq!(terminal_tab_row_at(95, g.terminal_split_y - 1, &g), None);
    // 同じ行でも、ターミナル列の外は当たらない。
    assert_eq!(terminal_tab_row_at(60, g.terminal_claude_y, &g), None);
}

#[test]
fn expand_button_hits_last_cols_of_each_column() {
    use crate::app::Focus;
    // main_area.x == 0 なので、worktreeボタンは [left_w-6, left_w-1) にまたがる。
    let g = geom(20, 50, 90);
    // worktreeボタン: 列 14..19。
    assert_eq!(g.expand_button_at(14), Some(Focus::Worktree));
    assert_eq!(g.expand_button_at(18), Some(Focus::Worktree));
    assert_eq!(g.expand_button_at(19), None); // btn_endは含まない
    assert_eq!(g.expand_button_at(13), None);
    // Explorerボタン: [left_end + explorer_w - 6, ...) = [44, 49)。
    assert_eq!(g.expand_button_at(44), Some(Focus::Explorer));
    assert_eq!(g.expand_button_at(48), Some(Focus::Explorer));
    // Viewerボタン: [explorer_end + viewer_w - 6, ...) = [84, 89)。
    assert_eq!(g.expand_button_at(84), Some(Focus::Viewer));
    assert_eq!(g.expand_button_at(88), Some(Focus::Viewer));
}

#[test]
fn expand_button_absent_for_narrow_columns() {
    // 幅7未満のカラムには展開ボタンがない。
    let g = geom(5, 50, 90);
    assert_eq!(g.expand_button_at(0), None);
    assert_eq!(g.expand_button_at(4), None);
}

#[test]
fn explorer_tree_row_at_rejects_the_border_row() {
    let g = geom(20, 50, 90); // main_area.y = 1 なので、上枠は行1。
    assert_eq!(explorer_tree_row_at(&g, 0, 30, 1), None);
    // 枠の内側の最初の行は表示上のインデックス0に解決される。
    assert_eq!(explorer_tree_row_at(&g, 0, 30, 2), Some(0));
}

#[test]
fn explorer_tree_row_at_rejects_columns_outside_the_explorer() {
    let g = geom(20, 50, 90);
    assert_eq!(explorer_tree_row_at(&g, 0, 19, 5), None); // worktree列
    assert_eq!(explorer_tree_row_at(&g, 0, 50, 5), None); // Viewer列
    // Explorer自身の端の列は依然として範囲内。
    assert_eq!(explorer_tree_row_at(&g, 0, 20, 5), Some(3));
    assert_eq!(explorer_tree_row_at(&g, 0, 49, 5), Some(3));
}

#[test]
fn explorer_tree_row_at_rejects_the_bottom_half() {
    let g = geom(20, 50, 90); // explorer_mid_y = 20
    assert_eq!(explorer_tree_row_at(&g, 0, 30, 18), Some(16)); // 実際に描画される最後の行
    assert_eq!(explorer_tree_row_at(&g, 0, 30, 19), None); // ツリー自身の下枠
    assert_eq!(explorer_tree_row_at(&g, 0, 30, 20), None); // ここからChanged filesが始まる
    assert_eq!(explorer_tree_row_at(&g, 0, 30, 25), None);
}

/// 両方のヒットテスタを、そのパネルが実際に描画する行数（レンダラが導出するのと
/// 同じ方法、2つの枠ぶんの height - 2）に結び付ける。単に「行Nはインデックス
/// Mに対応する」というだけのアサーションは、その関数がたまたまやっていることを
/// なぞるにすぎない。このテストは、どちらかのパネルが枠の行を受け入れたり
/// コンテンツ行を落としたりした瞬間に失敗する — これはまさに、画面に表示されて
/// いなかったファイルをクリックで開いてしまうという類のバグにつながるもの。
#[test]
fn row_at_helpers_accept_exactly_the_rows_their_panel_draws() {
    let g = geom(20, 50, 90);
    let col = 30;

    let tree_inner = (g.explorer_mid_y - g.main_area.y) as usize - 2;
    let tree_hits: Vec<usize> = (0..g.explorer_mid_y)
        .filter_map(|row| explorer_tree_row_at(&g, 0, col, row))
        .collect();
    assert_eq!(tree_hits, (0..tree_inner).collect::<Vec<_>>());

    let diff_bottom = g.main_area.y + g.main_area.height;
    let diff_inner = (diff_bottom - g.explorer_mid_y) as usize - 2;
    let diff_hits: Vec<usize> = (g.explorer_mid_y..diff_bottom)
        .filter_map(|row| diff_list_row_at(&g, 0, 0, col, row))
        .collect();
    assert_eq!(diff_hits, (0..diff_inner).collect::<Vec<_>>());
}

#[test]
fn explorer_tree_row_at_adds_the_scroll_offset() {
    let g = geom(20, 50, 90);
    assert_eq!(explorer_tree_row_at(&g, 5, 30, 2), Some(5));
    assert_eq!(explorer_tree_row_at(&g, 5, 30, 7), Some(10));
}

#[test]
fn diff_list_row_at_rejects_the_top_half_and_its_border() {
    let g = geom(20, 50, 90); // explorer_mid_y = 20
    assert_eq!(diff_list_row_at(&g, 0, 0, 30, 19), None); // まだファイルツリー
    assert_eq!(diff_list_row_at(&g, 0, 0, 30, 20), None); // diffリスト自身の上枠
    assert_eq!(diff_list_row_at(&g, 0, 0, 30, 21), Some(0)); // diffリストの最初の行
}

#[test]
fn diff_list_row_at_rejects_columns_outside_the_explorer() {
    let g = geom(20, 50, 90);
    assert_eq!(diff_list_row_at(&g, 0, 0, 19, 25), None); // worktree列
    assert_eq!(diff_list_row_at(&g, 0, 0, 50, 25), None); // Viewer列
    assert_eq!(diff_list_row_at(&g, 0, 0, 20, 25), Some(4));
    assert_eq!(diff_list_row_at(&g, 0, 0, 49, 25), Some(4));
}

#[test]
fn diff_list_row_at_rejects_the_bottom_border() {
    // main_area = Rect::new(0, 1, .., 40) → 下枠の行は 1 + 40 - 1 = 40 で、
    // これはリストの行ではなく「Ask Claude All」ボタンが置かれている場所。
    let g = geom(20, 50, 90);
    assert_eq!(diff_list_row_at(&g, 0, 0, 30, 39), Some(18)); // diffリストの最後の行
    assert_eq!(diff_list_row_at(&g, 0, 0, 30, 40), None);
    assert_eq!(diff_list_row_at(&g, 0, 0, 30, 45), None);
}

/// エラーバナーはリストの内側領域にエントリを1つ消費せずに収まるので、
/// エントリはその分だけ下の行から始まる。ここを間違えるとクリックが1ファイル分
/// ずれ、バナー自体が、スクロールして一番上に来ているものを開いてしまう。
/// クリックハンドラとホバートラッカーの両方がここを通るので、オフセットは
/// 一箇所だけ正しければよい。
#[test]
fn diff_list_row_at_skips_the_error_banner() {
    let g = geom(20, 50, 90); // explorer_mid_y = 20、最初の内側行は21
    assert_eq!(diff_list_row_at(&g, 0, 2, 30, 21), None); // エントリではなくバナー
    assert_eq!(diff_list_row_at(&g, 0, 2, 30, 22), None);
    assert_eq!(diff_list_row_at(&g, 0, 2, 30, 23), Some(0)); // 最初の実エントリ
    assert_eq!(diff_list_row_at(&g, 5, 2, 30, 23), Some(5)); // バナーの後にスクロール分
}

#[test]
fn diff_list_row_at_adds_the_scroll_offset() {
    let g = geom(20, 50, 90);
    assert_eq!(diff_list_row_at(&g, 5, 0, 30, 21), Some(5));
    assert_eq!(diff_list_row_at(&g, 5, 0, 30, 26), Some(10));
}

#[test]
fn double_click_within_threshold() {
    let t0 = Instant::now();
    let mut last = t0;
    // 前回のクリックから100ms後のクリックはダブルクリックになる。
    let is_double = register_double_click(&mut last, t0 + Duration::from_millis(100));
    assert!(is_double);
    assert_eq!(last, t0 + Duration::from_millis(100));
}

#[test]
fn single_click_beyond_threshold() {
    let t0 = Instant::now();
    let mut last = t0;
    // 400ms後のクリックはダブルクリックにならない（境界は含まない）。
    assert!(!register_double_click(
        &mut last,
        t0 + Duration::from_millis(400)
    ));
    // しきい値を大きく超えたクリックも同様。
    let t1 = t0 + Duration::from_millis(400);
    assert!(!register_double_click(
        &mut last,
        t1 + Duration::from_millis(500)
    ));
}

#[test]
fn indexed_double_click_requires_same_idx() {
    let t0 = Instant::now();
    let mut last = t0;
    let mut last_idx = 0usize;
    // idx 5への最初のクリック: 時間窓の中であっても、記録されているidx（0）と
    // 異なるのでダブルクリックにはならない。
    let first =
        register_double_click_on(&mut last, &mut last_idx, 5, t0 + Duration::from_millis(50));
    assert!(!first);
    assert_eq!(last_idx, 5);
    // 窓の中で同じidxへの2回目のクリック: ダブルクリックになる。
    let second =
        register_double_click_on(&mut last, &mut last_idx, 5, t0 + Duration::from_millis(100));
    assert!(second);
}

#[test]
fn indexed_double_click_resets_on_different_idx() {
    let t0 = Instant::now();
    let mut last = t0;
    let mut last_idx = 3usize;
    // 素早いクリックだが行が異なる → ダブルクリックにはならず、記録される
    // インデックス/時刻が更新されるので、次のクリックはこれと比較される。
    let hit = register_double_click_on(&mut last, &mut last_idx, 7, t0 + Duration::from_millis(10));
    assert!(!hit);
    assert_eq!(last_idx, 7);
    assert_eq!(last, t0 + Duration::from_millis(10));
}

// メニューバーのクリック

mod menu_clicks {
    use super::super::menu::{MenuClick, classify_menu_click};
    use crate::menu::state::{ItemHit, MenuFocus, MenuState};
    use ratatui::layout::Rect;

    const BAR_ROW: u16 = 1;

    /// 行1に2つのタイトルがあり、任意でメニュー1のドロップダウンが行2..6で
    /// 開いている状態。有効な行が1つ（3）、無効な行が1つ（4）ある。
    fn state(open: bool) -> MenuState {
        let mut bar_hits = crate::hit_map::ColumnSpans::default();
        bar_hits.push(0, 6, 0);
        bar_hits.push(6, 16, 1);
        let mut s = MenuState {
            bar_hits,
            ..Default::default()
        };
        if open {
            s.focus = MenuFocus::Open {
                index: 1,
                selected: 0,
                scroll: 0,
            };
            s.dropdown_area = Rect::new(6, 2, 20, 4);
            s.item_hits = vec![
                ItemHit {
                    y: 3,
                    item: 0,
                    enabled: true,
                },
                ItemHit {
                    y: 4,
                    item: 1,
                    enabled: false,
                },
            ];
        }
        s
    }

    #[test]
    fn clicking_a_title_opens_that_menu() {
        let s = state(false);
        assert_eq!(
            classify_menu_click(&s, Some(BAR_ROW), 8, BAR_ROW),
            MenuClick::Open(1)
        );
        assert_eq!(
            classify_menu_click(&s, Some(BAR_ROW), 2, BAR_ROW),
            MenuClick::Open(0)
        );
    }

    #[test]
    fn clicking_the_open_title_again_closes_it() {
        let s = state(true);
        assert_eq!(
            classify_menu_click(&s, Some(BAR_ROW), 8, BAR_ROW),
            MenuClick::Close,
            "the same title toggles rather than re-opening"
        );
    }

    #[test]
    fn clicking_a_different_title_switches_menus() {
        let s = state(true);
        assert_eq!(
            classify_menu_click(&s, Some(BAR_ROW), 2, BAR_ROW),
            MenuClick::Open(0)
        );
    }

    #[test]
    fn clicking_blank_bar_space_closes() {
        let s = state(true);
        assert_eq!(
            classify_menu_click(&s, Some(BAR_ROW), 40, BAR_ROW),
            MenuClick::Close
        );
    }

    #[test]
    fn clicking_an_enabled_row_activates_it() {
        let s = state(true);
        assert_eq!(
            classify_menu_click(&s, Some(BAR_ROW), 10, 3),
            MenuClick::Activate { menu: 1, item: 0 }
        );
    }

    #[test]
    fn clicking_a_disabled_row_does_nothing_but_keeps_the_menu_open() {
        let s = state(true);
        assert_eq!(
            classify_menu_click(&s, Some(BAR_ROW), 10, 4),
            MenuClick::Inert
        );
    }

    #[test]
    fn clicking_the_dropdown_border_is_inert() {
        // 行2と5はポップアップ自身の枠の行: 矩形の内側だが、項目のヒットは
        // 持たない。
        let s = state(true);
        assert_eq!(
            classify_menu_click(&s, Some(BAR_ROW), 10, 2),
            MenuClick::Inert
        );
        assert_eq!(
            classify_menu_click(&s, Some(BAR_ROW), 10, 5),
            MenuClick::Inert
        );
    }

    #[test]
    fn clicking_outside_an_open_menu_closes_and_swallows_the_click() {
        let s = state(true);
        assert_eq!(
            classify_menu_click(&s, Some(BAR_ROW), 60, 30),
            MenuClick::Close,
            "the dismissing click must not also reach the panel underneath"
        );
    }

    #[test]
    fn clicks_elsewhere_pass_through_when_no_menu_is_open() {
        let s = state(false);
        assert_eq!(
            classify_menu_click(&s, Some(BAR_ROW), 60, 30),
            MenuClick::Pass
        );
    }

    #[test]
    fn a_stale_dropdown_rect_cannot_swallow_clicks_after_closing() {
        // リグレッション防止: close()は記録された領域をクリアするので、
        // 最後の描画時の矩形がクリックを飲み込み続けることはない。
        let mut s = state(true);
        s.close();
        assert_eq!(
            classify_menu_click(&s, Some(BAR_ROW), 10, 3),
            MenuClick::Pass
        );
    }

    #[test]
    fn the_bar_row_is_claimed_even_with_no_menu_open() {
        // そうしないと、クリックはhandle_title_bar_clickまで素通りしてしまい、
        // そちらはmain area より上のすべての行を消費してしまう。
        let s = state(false);
        assert!(matches!(
            classify_menu_click(&s, Some(BAR_ROW), 8, BAR_ROW),
            MenuClick::Open(_)
        ));
    }
}

/// 折りたたみマーカーの当たり判定。三角の1列だけだと狙えないので、行番号より
/// 右のガターをまとめて受ける — ただし行番号（コメント作成）とガターの外
/// （バッジの「+」）までは広げない。
#[test]
fn the_fold_marker_claims_the_gutter_right_of_the_line_number() {
    // ガターは [10, 20)。隙間が 15、マーカーが 16、隙間が 17、区切り線が 18、
    // 空白が 19。
    let gutter_end = 20;
    assert!(
        !in_fold_zone(14, gutter_end),
        "行番号の最終桁はコメント側の担当"
    );
    assert!(in_fold_zone(15, gutter_end));
    assert!(in_fold_zone(16, gutter_end));
    assert!(in_fold_zone(17, gutter_end));
    assert!(in_fold_zone(18, gutter_end));
    assert!(in_fold_zone(19, gutter_end));
    assert!(!in_fold_zone(20, gutter_end), "バッジ列はコメント側の担当");
}
