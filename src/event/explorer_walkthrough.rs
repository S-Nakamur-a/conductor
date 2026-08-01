//! Explorer walkthrough-view key handling (the AI walkthrough shown in the
//! Explorer's bottom pane, see `viewer::ExplorerBottomView::Walkthrough`).

use crossterm::event::KeyEvent;

use crate::app::App;
use crate::keymap::{Action, KeyContext};

use super::adjust_walkthrough_scroll;

/// Handle keys while the Explorer's Walkthrough view is focused.
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
        // Reuses the comment list's detail-overlay action name (see
        // default_keybinds.toml's `[layers.explorer_walkthrough]`) — the
        // walkthrough view's own detail overlay, not the comment one.
        Some(Action::ViewCommentDetail) if len > 0 => {
            app.viewer_state.explorer.walkthrough_detail_active = true;
        }
        _ => {}
    }

    adjust_walkthrough_scroll(app);
}
