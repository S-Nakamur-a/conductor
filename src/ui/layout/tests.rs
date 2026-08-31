//! LayoutCache::update と accordion_widths のテスト。

use ratatui::layout::Rect;

use super::{LayoutCache, accordion_widths};
use crate::config::LayoutConfig;
use crate::types::Focus;

/// 指定した比率で最小限の LayoutConfig を組み立てる。
fn layout(explorer: u16, viewer: u16, terminal: u16) -> LayoutConfig {
    LayoutConfig {
        explorer_width_pct: explorer,
        viewer_width_pct: viewer,
        terminal_split_pct: terminal,
        explorer_split_pct: 50,
    }
}

/// 非自明なレイアウト分割を発生させるのに十分大きい、非ゼロの Rect。
fn rect(w: u16, h: u16) -> Rect {
    Rect::new(0, 0, w, h)
}

#[test]
fn 初回のupdateはtrueを返す() {
    let mut cache = LayoutCache::default();
    let changed = cache.update(rect(200, 50), None, false, &layout(24, 38, 80), 80);
    assert!(changed, "first update must recompute");
}

#[test]
fn 同じ入力の2回目はfalseを返す() {
    let mut cache = LayoutCache::default();
    let cfg = layout(24, 38, 80);
    cache.update(rect(200, 50), None, false, &cfg, 80);
    let changed = cache.update(rect(200, 50), None, false, &cfg, 80);
    assert!(!changed, "identical inputs must not recompute");
}

#[test]
fn 入力のどれが動いてもキャッシュは無効になる() {
    // 入力のどれが動いてもキャッシュは無効化されなければならない。ターミナル分割だけは
    // 実行時パラメータ (shell の拡大縮小) なので config ではなく引数の方を変える。
    let base = layout(24, 38, 80);
    let mut explorer_split = layout(24, 38, 80);
    explorer_split.explorer_split_pct = 30;
    let cases: [(&str, Rect, LayoutConfig, u16); 5] = [
        ("frame_area", rect(201, 50), layout(24, 38, 80), 80),
        ("explorer_width_pct", rect(200, 50), layout(30, 38, 80), 80),
        ("viewer_width_pct", rect(200, 50), layout(24, 42, 80), 80),
        ("terminal_split_pct", rect(200, 50), layout(24, 38, 80), 70),
        ("explorer_split_pct", rect(200, 50), explorer_split, 80),
    ];
    for (label, area, cfg, split) in cases {
        let mut cache = LayoutCache::default();
        let mut first = base.clone();
        first.explorer_split_pct = 50;
        cache.update(rect(200, 50), None, false, &first, 80);
        assert!(
            cache.update(area, None, false, &cfg, split),
            "{label} change must invalidate"
        );
    }
}

#[test]
fn 大きすぎる割合でもaccordion_widthsは落ちない() {
    // 100 を超える割合でも panic してはならない（Percentage 制約側でクランプされる）。
    let _ = accordion_widths(None, 200, 240, 0);
    let _ = accordion_widths(None, 100, 60, 60);
}

#[test]
fn terminal_split_pctが100を超えても落ちない() {
    // terminal_split_pct が 100 を超えると shell_pct は 0 に飽和するが panic してはならない。
    let mut cache = LayoutCache::default();
    let _ = cache.update(rect(200, 50), None, false, &layout(24, 38, 200), 200);
}

#[test]
fn 最大化したエディタはexplorerの枠で全幅を取る() {
    // render_ui は explorer+viewer カラムを editor 領域として統合するので、
    // 最大化したエディタは explorer 側の枠に全幅を割り当て（viewer は0）、
    // ターミナルカラムは残りゼロに縮む。
    let (left, explorer, viewer) = accordion_widths(Some(Focus::Editor), 120, 24, 38);
    assert_eq!((left, explorer, viewer), (0, 120, 0));
}

#[test]
fn 既定のレイアウトは設定の割合を使う() {
    // デフォルトの割合（24/38）、幅200カラムのターミナルの場合。
    let (left, explorer, viewer) = accordion_widths(None, 200, 24, 38);
    assert_eq!(left, 0, "worktree column is always hidden");
    assert_eq!(explorer, 48, "200 * 24 / 100 = 48");
    assert_eq!(viewer, 76, "200 * 38 / 100 = 76");
}

#[test]
fn 指定した割合が尊重される() {
    // explorer を広く（30%）、viewer を狭く（30%）。
    let (left, explorer, viewer) = accordion_widths(None, 100, 30, 30);
    assert_eq!(left, 0);
    assert_eq!(explorer, 30);
    assert_eq!(viewer, 30);
}

// メニューバーの行

#[test]
fn メニューバーはタイトルバーの直下に来る() {
    let mut cache = LayoutCache::default();
    cache.update(rect(200, 50), None, false, &layout(24, 38, 80), 80);
    assert_eq!(cache.title_area.y, 0);
    assert_eq!(cache.title_area.height, 1);
    assert_eq!(
        cache.menubar_area.y,
        cache.title_area.y + cache.title_area.height,
        "the menu bar must be the row immediately below the title bar"
    );
    assert_eq!(cache.menubar_area.height, 1);
    assert_eq!(
        cache.menubar_area.width, 200,
        "the bar spans the full width so its blank stretch is still clickable"
    );
}

#[test]
fn パネル最大化中もメニューバーは残る() {
    // worktree ストリップは折りたたまれて最大化パネルに行を返すが、こちらは違う。
    // 最大化解除後にしか開けないメニューは、使われなくなるメニューである。
    let mut cache = LayoutCache::default();
    cache.update(
        rect(200, 50),
        Some(Focus::Explorer),
        false,
        &layout(24, 38, 80),
        80,
    );
    assert_eq!(cache.menubar_area.height, 1);
    assert_eq!(cache.wtbar_area.height, 0, "worktree strip still collapses");
}

#[test]
fn メニューバーの1行はmain_areaから引かれる() {
    // このバーはステータスバーから行を取るのでも worktree ストリップに
    // 上書き描画するのでもなく、コンテンツ領域から行を取る。
    let mut cache = LayoutCache::default();
    let cfg = layout(24, 38, 80);
    cache.update(rect(200, 50), None, false, &cfg, 80);

    assert_eq!(
        cache.main_area.y,
        cache.wtbar_area.y + cache.wtbar_area.height
    );
    assert_eq!(
        cache.main_area.y + cache.main_area.height,
        cache.status_area.y,
        "main area must still butt up against the status bar"
    );
    // title(1) + menubar(1) + wtbar(1) + status(1) = クロム部分の合計4行。
    assert_eq!(cache.main_area.height, 50 - 4);
}

#[test]
fn 縦の行は1行残らず割り当てられる() {
    // 積み重なった各領域の間に隙間も重複もないことを、main area が圧迫されるほど
    // 短い高さも含むいくつかの高さで確認する。
    for h in [50_u16, 10, 6] {
        let mut cache = LayoutCache::default();
        cache.update(rect(120, h), None, false, &layout(24, 38, 80), 80);
        let total = cache.title_area.height
            + cache.menubar_area.height
            + cache.wtbar_area.height
            + cache.main_area.height
            + cache.status_area.height;
        assert_eq!(
            total, h,
            "regions must tile the frame exactly at height {h}"
        );
    }
}
