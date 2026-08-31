//! render/ 配下の描画関数が返す値の型。
//!
//! いずれも描画してみるまで決まらない値（画面上のヒット領域、折り返し後の
//! 総行数に対してクランプしたスクロール位置）を運ぶだけで、状態への書き戻しは
//! 呼び出し側（[crate::viewer::panel]）が行う。

use crate::hit_map::ColumnSpans;
use crate::ui::tab_bar::TabAction;
use crate::viewer::ScreenRow;
use ratatui::layout::Rect;

/// [super::render] 1回分の呼び出しの結果。summary は呼び出し側が別経路
/// （[super::render_summary_view]）で描くのでここには含まれず、残りのプレーン/
/// diff/markdown のどの分岐を描いたかによって埋まるフィールドが変わるので、
/// 描かなかった分岐のぶんは None のままになる。
#[derive(Default)]
pub(in crate::viewer) struct RenderOutcome {
    pub screen_row_map: Option<Vec<ScreenRow>>,
    pub screen_entry_map: Option<Vec<Option<usize>>>,
    pub tab_row: Option<TabRowOutcome>,
    pub markdown_scroll: Option<ScrollOutcome>,
}

/// タブ行のクリック領域と、開いているタブがはみ出している場合に解決された
/// 表示開始位置。
pub(in crate::viewer) struct TabRowOutcome {
    pub hits: ColumnSpans<TabAction>,
    pub scroll: usize,
}

/// 折り返し/整形後の総行数に対してクランプしたスクロール位置。総行数は
/// 折り返し幅（パネル幅）が決まらないと分からず、スクロールはその総行数を
/// 超えないよう毎回クランプし直すので、両方とも描画結果としてしか出てこない。
pub(in crate::viewer) struct ScrollOutcome {
    pub total_lines: usize,
    pub scroll: usize,
}

/// [super::diff_view::render_diff_view] の結果。
pub(in crate::viewer) struct DiffViewOutcome {
    pub screen_row_map: Vec<ScreenRow>,
    pub screen_entry_map: Vec<Option<usize>>,
    pub tab_row: Option<TabRowOutcome>,
}

/// [super::hover::render_hover_info_overlay] 1回分の呼び出しの結果。
pub(in crate::viewer) struct HoverOutcome {
    pub base: BaseOutcome,
    pub refs: Option<RefsOutcome>,
    pub preview_rect: Option<Rect>,
}

/// シグネチャ/doc ポップアップのクリック領域。
pub(in crate::viewer) struct BaseOutcome {
    pub info_rect: Rect,
    pub refs_hit: Rect,
    pub def_hit: Rect,
}

/// 参照一覧ポップアップのクリック領域と、選択中の項目を可視窓に収めた後の
/// スクロール位置。
pub(in crate::viewer) struct RefsOutcome {
    pub rect: Rect,
    pub row_hits: Vec<(usize, Rect)>,
    pub scroll: usize,
}
