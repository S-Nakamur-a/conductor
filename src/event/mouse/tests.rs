//! マウスのヒットテストジオメトリと、左マージンのクリック分類。

use super::{ClickGeometry, Column, in_fold_zone, terminal_tab_row_at};
use crate::viewer::mouse::{
    MarginClickAction, MarginZone, classify_margin_click, thread_anchor_line,
};

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
fn ガターとバッジのクリックは必ずコメントを始める() {
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
fn マーカーのクリックは既存スレッドへ寄せる() {
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
fn テスト行のバッジを押すとテストが走る() {
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
fn 範囲の途中は最も近い終了行へ振り替える() {
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
fn 縦の境界は2セルとも当たる() {
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
fn 横の分割はその列の中で当たる() {
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
fn 角では縦の境界が勝つ() {
    use crate::app::Divider;
    // (explorer_end-1, explorer_mid_y) は縦の境界セルであると同時にExplorerの
    // 分割線の行でもある — 縦の境界が優先されなければならない。
    let g = geom(0, 24, 62);
    assert_eq!(g.divider_at(23, 20), Some(Divider::ExplorerViewer));
}

#[test]
fn パネルの内側では境界に当たらない() {
    let g = geom(0, 24, 62);
    assert_eq!(g.divider_at(10, 10), None);
    assert_eq!(g.divider_at(70, 10), None);
}

#[test]
fn 桁は境界で列に振り分けられる() {
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
fn terminalのタブ行は帯の2行だけ() {
    let g = geom(20, 50, 90);
    assert_eq!(terminal_tab_row_at(95, g.terminal_claude_y, &g), Some(true));
    assert_eq!(terminal_tab_row_at(95, g.terminal_split_y, &g), Some(false));
    assert_eq!(terminal_tab_row_at(95, g.terminal_split_y - 1, &g), None);
    // 同じ行でも、ターミナル列の外は当たらない。
    assert_eq!(terminal_tab_row_at(60, g.terminal_claude_y, &g), None);
}

#[test]
fn 展開ボタンは各列の末尾の桁に当たる() {
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
fn 狭い列には展開ボタンが出ない() {
    // 幅7未満のカラムには展開ボタンがない。
    let g = geom(5, 50, 90);
    assert_eq!(g.expand_button_at(0), None);
    assert_eq!(g.expand_button_at(4), None);
}

// メニューバーのクリック

mod menu_clicks {
    use crate::menu::mouse::{MenuClick, classify_menu_click};
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
    fn タイトルを押すとそのメニューが開く() {
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
    fn 開いているタイトルをもう一度押すと閉じる() {
        let s = state(true);
        assert_eq!(
            classify_menu_click(&s, Some(BAR_ROW), 8, BAR_ROW),
            MenuClick::Close,
            "the same title toggles rather than re-opening"
        );
    }

    #[test]
    fn 別のタイトルを押すとメニューが切り替わる() {
        let s = state(true);
        assert_eq!(
            classify_menu_click(&s, Some(BAR_ROW), 2, BAR_ROW),
            MenuClick::Open(0)
        );
    }

    #[test]
    fn バーの空白を押すと閉じる() {
        let s = state(true);
        assert_eq!(
            classify_menu_click(&s, Some(BAR_ROW), 40, BAR_ROW),
            MenuClick::Close
        );
    }

    #[test]
    fn 有効な行を押すと実行される() {
        let s = state(true);
        assert_eq!(
            classify_menu_click(&s, Some(BAR_ROW), 10, 3),
            MenuClick::Activate { menu: 1, item: 0 }
        );
    }

    #[test]
    fn 無効な行を押しても何も起きずメニューは開いたまま() {
        let s = state(true);
        assert_eq!(
            classify_menu_click(&s, Some(BAR_ROW), 10, 4),
            MenuClick::Inert
        );
    }

    #[test]
    fn ドロップダウンの枠を押しても何も起きない() {
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
    fn メニューの外を押すと閉じてクリックを飲む() {
        let s = state(true);
        assert_eq!(
            classify_menu_click(&s, Some(BAR_ROW), 60, 30),
            MenuClick::Close,
            "the dismissing click must not also reach the panel underneath"
        );
    }

    #[test]
    fn メニューが閉じていればクリックは素通しする() {
        let s = state(false);
        assert_eq!(
            classify_menu_click(&s, Some(BAR_ROW), 60, 30),
            MenuClick::Pass
        );
    }

    #[test]
    fn 閉じたあとの古い矩形はクリックを飲まない() {
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
    fn バーの行はメニューが閉じていても取る() {
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
fn 畳みマーカーは行番号の右のガターを取る() {
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
