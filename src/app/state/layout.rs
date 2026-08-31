//! パネルの幾何: キャッシュしたレイアウト矩形と、境界のドラッグリサイズ。

use crate::app::panel_resize::Divider;
use crate::types::Focus;
use crate::ui::layout::LayoutCache;

/// 3 カラムアコーディオンの配分と、マウスによる境界リサイズの状態。
///
/// cache は描画時に計算され、マウスハンドラが読む。境界のドラッグ判定は
/// その矩形に対して行うので、キャッシュとドラッグ状態を同じ構造体に置いている。
#[derive(Default)]
pub struct PanelLayout {
    /// 直近に計算したレイアウト矩形 (フレームサイズか最大化状態が変わると再計算)。
    pub cache: LayoutCache,
    /// 100% に拡大しているパネル。None は通常レイアウト。
    pub expanded: Option<Focus>,
    /// ターミナルカラム内で Claude Code 側が占める高さの割合 (残りが Shell)。
    ///
    /// 起動時は config.layout.terminal_split_pct から。tmux 風のペイン
    /// リサイズ (ターミナルにフォーカスして Ctrl+Alt+上下) で実行時に変わり、
    /// 再起動後も残るよう config に書き戻される。
    pub terminal_split_pct: u16,
    /// いまマウスでドラッグ中の境界。
    ///
    /// 境界上での mouse-down で立ち、drag のたびに動き、mouse-up で
    /// (config への保存 1 回とともに) 消える。Some のあいだ、drag イベントは
    /// 各パネル本来の処理ではなくリサイズになる。
    pub divider_drag: Option<Divider>,
    /// マウスが乗っている境界。
    ///
    /// リサイズ可能であることの手掛かり — 端末では OS のカーソル形状を
    /// col-resize に変えられないので、境界をハイライトすることで代用する。
    /// 描画時はドラッグ中の境界がホバーより優先される。
    pub divider_hover: Option<Divider>,
}
