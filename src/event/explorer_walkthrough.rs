//! Explorer walkthrough ビューのキー処理（Explorer 下部ペインに表示される AI walkthrough、
//! viewer::ExplorerBottomView::Walkthrough を参照）。

use crossterm::event::KeyEvent;

use crate::app::App;
use crate::keymap::{Action, KeyContext};

use super::adjust_walkthrough_scroll;

/// Explorer の Walkthrough ビューにフォーカスがある間のキー処理。
pub(super) fn handle_explorer_walkthrough_key(app: &mut App, key: KeyEvent) {
    let len = app
        .walkthrough.current
        .as_ref()
        .map(|wt| wt.steps.len())
        .unwrap_or(0);
    let action = app.keymap.resolve(&key, KeyContext::ExplorerWalkthrough);

    match action {
        Some(Action::ExitSubPanel) => {
            app.viewer_state.explorer.explorer_focus_on_diff_list = false;
        }
        Some(Action::NavigateDown) => app.walkthrough_move(1),
        Some(Action::NavigateUp) => app.walkthrough_move(-1),
        Some(Action::GoToTop) => app.viewer_state.explorer.walkthrough_selected = 0,
        Some(Action::GoToBottom) if len > 0 => {
            app.viewer_state.explorer.walkthrough_selected = len - 1;
        }
        Some(Action::Select) => app.walkthrough_jump_selected(),
        Some(Action::WalkthroughNextStep) => app.walkthrough_step(1),
        Some(Action::WalkthroughPrevStep) => app.walkthrough_step(-1),
        // コメントリストの detail-overlay アクション名を流用している (default_keybinds.toml の
        // [layers.explorer_walkthrough] を参照)。開くのは walkthrough ビュー自身の detail overlay
        // であり、コメントの detail overlay ではない。
        Some(Action::ViewCommentDetail) if len > 0 => {
            app.viewer_state.explorer.walkthrough_detail_active = true;
        }
        _ => {}
    }

    adjust_walkthrough_scroll(app);
}
