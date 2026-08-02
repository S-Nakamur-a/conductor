//! UI モジュール — TUI の描画全体を統括する。
//!
//! 各サブモジュールは統合レイアウト内の1つのパネルに対応する。

use crate::app::{App, Focus};
use crate::theme::Theme;

pub mod common;
pub mod decoration;
pub mod editor_panel;
pub mod explorer_panel;
pub mod markdown;
pub mod menu_bar;
pub mod party;
pub mod reflow_view;
pub mod rich;
pub mod tab_bar;
pub mod terminal_claude;
pub mod terminal_shell;
pub mod viewer_panel;
pub mod walkthrough_pane;
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

/// UI 描画のために App から抽出した、共有の読み取り専用コンテキスト。
///
/// App 構造体全体への参照を要求せずに、ほぼすべての描画関数が必要とする
/// 共通フィールドを提供する。
#[allow(dead_code)]
pub struct RenderContext<'a> {
    pub theme: &'a Theme,
    pub focus: Focus,
    pub expanded_panel: Option<Focus>,
    pub ui_tick: u64,
}

#[allow(dead_code)]
impl<'a> RenderContext<'a> {
    pub fn from_app(app: &'a App) -> Self {
        Self {
            theme: &app.theme,
            focus: app.focus,
            expanded_panel: app.expanded_panel,
            ui_tick: app.ui_tick,
        }
    }

    /// 指定したパネルが現在フォーカスされているかどうか。
    pub fn is_focused(&self, panel: Focus) -> bool {
        self.focus == panel
    }

    /// 指定したパネルが現在画面全体に拡大表示されているかどうか。
    pub fn is_expanded(&self, panel: Focus) -> bool {
        self.expanded_panel == Some(panel)
    }
}
