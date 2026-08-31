//! UI モジュール — TUI の描画全体を統括する。
//!
//! 各サブモジュールは統合レイアウト内の1つのパネルに対応する。

pub mod chrome;
pub mod common;
pub mod markdown;
pub mod tab_bar;

// トップレベルのレイアウト統括（render_ui, accordion_widths）。
pub mod layout;

// オーバーレイの描画（layout::render_ui のオーバーレイから呼ばれる）。
pub mod dashboard;
pub mod grep_search;
pub mod panel_overlay;
pub mod review;
pub mod theme_picker;
