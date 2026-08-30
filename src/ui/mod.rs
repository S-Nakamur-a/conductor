//! UI モジュール — TUI の描画全体を統括する。
//!
//! 各サブモジュールは統合レイアウト内の1つのパネルに対応する。

pub mod common;
pub mod decoration;
pub mod editor_panel;
pub mod markdown;
pub mod tab_bar;
pub mod terminal_claude;
pub mod terminal_shell;
pub mod viewer_panel;
pub mod worktree_bar;
pub mod worktree_panel;

// トップレベルのレイアウト統括（render_ui, accordion_widths）。
pub mod layout;

// オーバーレイの描画（layout::render_ui のオーバーレイから呼ばれる）。
pub mod dashboard;
pub mod grep_search;
pub mod hover_info;
pub mod panel_overlay;
pub mod references;
pub mod review;
pub mod symbol_action;
pub mod theme_picker;
