//! Explorer が外から借りるもの。
//!
//! パネルが [crate::app::App] 型を見ないのは、依存を数えられる形にしておくため。
//! ここのフィールド数がそのまま Explorer の結合度で、増えれば差分に出る。
//! すべて共有借用なので、Explorer がここを書き換える経路は存在しない。

use crate::config::Config;
use crate::diff_state::DiffState;
use crate::keymap::KeyMap;
use crate::review_state::ReviewState;
use crate::theme::Theme;

/// 描画と入力の両方が要るもの。
pub struct Ctx<'a> {
    pub theme: &'a Theme,
    pub config: &'a Config,
    pub keymap: &'a KeyMap,
    /// Explorer にフォーカスがあるか。ペインのどちらかは Explorer 自身が持つ。
    pub focused: bool,
    /// 下ペインの diff 一覧が並べるもの。
    pub diff: &'a DiffState,
    /// 下ペインのコメント一覧が並べるもの。
    pub review: &'a ReviewState,
    /// 変更ファイル一覧の状態チップと、ツリーのタイトルの印。
    pub revidere: crate::revidere::ArtifactState,
}

/// 描画にだけ要る、時間で変わるもの。入力側は見ない。
pub struct Paint<'a> {
    /// どの行が hover されているか。位相は行ごとに引く。
    pub hover_tree: &'a crate::widget::row::HoverRow,
    pub hover_changes: &'a crate::widget::row::HoverRow,
    /// 状態チップに下線を引くか。
    pub revidere_badge_hover: bool,
    /// ペインの枠の色。フォーカス切替の補間が済んだ値を受け取る。
    /// 補間には切替時刻が要るが、それを持つのは App なので計算も App が行う。
    pub border: ratatui::style::Color,
    /// revidere チップの回転。
    pub tick: u64,
    /// 検索欄を出すか。Viewer の検索状態をそのまま映す。
    pub search: Option<&'a str>,
    /// パネルが最大化されているか。`[<=>]` ボタンの向きに使う。
    pub expanded: bool,
}
