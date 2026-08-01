//! Mouse handling for the menu bar and its dropdown.
//!
//! Both entry points run *before* the other bar handlers in
//! [`handle_mouse_event`](super::handle_mouse_event). That order is load
//! bearing: `handle_title_bar_click` treats every row above `main_area` as its
//! own and returns `true` unconditionally, so a menu-bar click placed after it
//! would never be seen.
//!
//! The decision of *what* a click means is [`classify_menu_click`], a pure
//! function over the recorded hit regions — same shape as
//! [`classify_margin_click`](super::viewer_panel::classify_margin_click), and
//! for the same reason: the interesting rules (toggle, dismiss, inert row) are
//! then testable without standing up an `App` or a terminal.

use crate::app::App;
use crate::menu::MenuFocus;
use crate::menu::model::MENUS;
use crate::menu::state::MenuState;

/// What a left click at a given point should do to the menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MenuClick {
    /// Run `item` of `menu`.
    Activate { menu: usize, item: usize },
    /// Open `menu`'s dropdown.
    Open(usize),
    /// Leave the menu entirely.
    Close,
    /// Consumed, but nothing happens — a disabled row, a separator, or the
    /// dropdown's own border. Keeps the menu open under a near-miss instead of
    /// blinking it shut.
    Inert,
    /// Not the menu's click; let the rest of the dispatcher have it.
    Pass,
}

/// Items of the menu at `index`, or an empty slice if the index is stale.
fn items_of(index: usize) -> &'static [crate::menu::MenuItem] {
    MENUS.get(index).map(|m| m.items).unwrap_or(&[])
}

/// Decide what a click at `(col, row)` means. `bar_row` is the menu bar's
/// screen row, or `None` when the bar isn't drawn.
pub(super) fn classify_menu_click(
    state: &MenuState,
    bar_row: Option<u16>,
    col: u16,
    row: u16,
) -> MenuClick {
    // Inside the open dropdown.
    if state.in_dropdown(col, row) {
        return match (state.focus.open_index(), state.item_hit_at(row)) {
            (Some(menu), Some(hit)) if hit.enabled => MenuClick::Activate {
                menu,
                item: hit.item,
            },
            _ => MenuClick::Inert,
        };
    }

    // On the bar row itself.
    if bar_row == Some(row) {
        return match state.bar_hit_at(col) {
            // Clicking the open menu closes it, so one target toggles rather
            // than re-opening what is already there.
            Some(idx) if state.focus.open_index() == Some(idx) => MenuClick::Close,
            Some(idx) => MenuClick::Open(idx),
            // Blank stretch of the bar: nothing to open, and any open menu goes
            // away.
            None => MenuClick::Close,
        };
    }

    // Anywhere else: dismiss if a menu is up, otherwise not our event. The
    // dismissing click is swallowed on purpose — closing a menu should not also
    // press whatever sat underneath it.
    if state.focus.is_active() {
        MenuClick::Close
    } else {
        MenuClick::Pass
    }
}

/// Handle a left click against the menu bar and any open dropdown. Returns
/// `true` when the click was consumed.
pub(super) fn handle_menu_click(app: &mut App, col: u16, row: u16) -> bool {
    let bar = app.layout.cache.menubar_area;
    let bar_row = (bar.height > 0).then_some(bar.y);

    match classify_menu_click(&app.menu, bar_row, col, row) {
        MenuClick::Activate { menu, item } => {
            super::super::menu::activate_item(app, menu, item);
            true
        }
        MenuClick::Open(idx) => {
            app.menu.open(idx, items_of(idx));
            true
        }
        MenuClick::Close => {
            app.menu.close();
            true
        }
        MenuClick::Inert => true,
        MenuClick::Pass => false,
    }
}

/// Track hover for the menu bar. Returns `true` when the menu owns this
/// movement, so the caller skips the other panels' hover bookkeeping (whatever
/// is under the dropdown shouldn't light up).
pub(super) fn handle_menu_hover(app: &mut App, col: u16, row: u16) -> bool {
    let bar = app.layout.cache.menubar_area;
    let on_bar = bar.height > 0 && row == bar.y;

    // Highlight the title under the cursor; resolving to `None` off the row
    // doubles as the "mouse left the bar" clear.
    app.menu.hover = if on_bar { app.menu.bar_hit_at(col) } else { None };

    match app.menu.focus {
        MenuFocus::Closed => false,

        // With a menu open, sliding along the bar switches which one is shown —
        // the behaviour that makes a menu bar browsable rather than a series of
        // click-open-click-close trips.
        MenuFocus::Open { index, .. } => {
            if on_bar {
                if let Some(idx) = app.menu.bar_hit_at(col)
                    && idx != index
                {
                    app.menu.open(idx, items_of(idx));
                }
            } else if app.menu.in_dropdown(col, row) {
                // Hovering a row moves the selection, so the keyboard and the
                // pointer share one notion of "current item".
                if let Some(hit) = app.menu.item_hit_at(row)
                    && let MenuFocus::Open {
                        ref mut selected, ..
                    } = app.menu.focus
                {
                    *selected = hit.item;
                }
            }
            true
        }

        // Bar focused via F10 but nothing dropped down. Hover only highlights;
        // opening still takes a click or Down/Enter, so drifting the mouse
        // across the top of the screen can't pop a menu the user never asked
        // for.
        MenuFocus::Bar { .. } => on_bar,
    }
}
