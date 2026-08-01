//! Keyboard handling while the menu bar has focus.
//!
//! Once the menu is active it consumes every key, so the letter-jump shortcuts
//! here can use bare characters without colliding with any panel binding.
//!
//! `Esc` unwinds one level at a time (dropdown → bar → app) rather than
//! dismissing everything at once, matching how the app's other nested modals
//! behave and giving a mistyped `Down` an obvious way back.

use crossterm::event::{KeyCode, KeyEvent};

use crate::app::App;
use crate::menu::model::{MENUS, MenuItem};
use crate::menu::state::{
    MenuFocus, find_by_initial, first_selectable, last_selectable, step_menu, step_selection,
};

/// Items of the menu at `index`, or an empty slice if the index is stale.
fn items_of(index: usize) -> &'static [MenuItem] {
    MENUS.get(index).map(|m| m.items).unwrap_or(&[])
}

/// Index of the first menu whose title starts with `ch`, case-insensitively.
fn menu_by_initial(ch: char) -> Option<usize> {
    let target = ch.to_ascii_lowercase();
    MENUS.iter().position(|m| {
        m.title
            .chars()
            .next()
            .is_some_and(|c| c.to_ascii_lowercase() == target)
    })
}

/// Run the command on `item_idx` of menu `menu_idx`, if it has one and it is
/// currently available.
fn activate(app: &mut App, menu_idx: usize, item_idx: usize) {
    let Some(id) = items_of(menu_idx).get(item_idx).and_then(MenuItem::command) else {
        return;
    };
    // A greyed-out row stays selectable so its existence is visible, but
    // activating it does nothing — the same as clicking a disabled GUI item.
    if !crate::menu::command_enabled(id, app) {
        return;
    }
    // Close first: commands like `OpenRepo` push an overlay, and closing
    // afterwards would clear the state they just set up.
    app.menu.close();
    app.execute_palette_command(id);
}

/// Keep the highlighted row inside the visible window of the dropdown.
fn rescroll(app: &mut App) {
    let visible = crate::ui::menu_bar::visible_rows(app, app.layout.cache.frame_area.height);
    app.menu.scroll_selection_into_view(visible);
}

/// Handle a key while [`MenuFocus`] is active. The caller has already
/// established that the menu owns input.
pub(super) fn handle_menu_key(app: &mut App, key: KeyEvent) {
    match app.menu.focus {
        MenuFocus::Closed => {}

        // ── Bar focused, nothing dropped down ────────────────────────────
        MenuFocus::Bar { index } => match key.code {
            KeyCode::Esc => app.menu.close(),
            KeyCode::Left => app.menu.focus_bar(step_menu(MENUS.len(), index, -1)),
            KeyCode::Right => app.menu.focus_bar(step_menu(MENUS.len(), index, 1)),
            KeyCode::Home => app.menu.focus_bar(0),
            KeyCode::End => app.menu.focus_bar(MENUS.len().saturating_sub(1)),
            KeyCode::Down | KeyCode::Enter | KeyCode::Char(' ') => {
                app.menu.open(index, items_of(index));
            }
            // A letter jumps straight to that menu and opens it — the fastest
            // route for someone who knows where they are going.
            KeyCode::Char(c) => {
                if let Some(idx) = menu_by_initial(c) {
                    app.menu.open(idx, items_of(idx));
                }
            }
            _ => {}
        },

        // ── Dropdown open ────────────────────────────────────────────────
        MenuFocus::Open {
            index,
            selected,
            scroll,
        } => match key.code {
            KeyCode::Esc => app.menu.focus_bar(index),
            KeyCode::Left => {
                let idx = step_menu(MENUS.len(), index, -1);
                app.menu.open(idx, items_of(idx));
            }
            KeyCode::Right => {
                let idx = step_menu(MENUS.len(), index, 1);
                app.menu.open(idx, items_of(idx));
            }
            KeyCode::Up => {
                app.menu.focus = MenuFocus::Open {
                    index,
                    selected: step_selection(items_of(index), selected, -1),
                    scroll,
                };
                rescroll(app);
            }
            KeyCode::Down => {
                app.menu.focus = MenuFocus::Open {
                    index,
                    selected: step_selection(items_of(index), selected, 1),
                    scroll,
                };
                rescroll(app);
            }
            KeyCode::Home => {
                app.menu.focus = MenuFocus::Open {
                    index,
                    selected: first_selectable(items_of(index)),
                    scroll,
                };
                rescroll(app);
            }
            KeyCode::End => {
                app.menu.focus = MenuFocus::Open {
                    index,
                    selected: last_selectable(items_of(index)),
                    scroll,
                };
                rescroll(app);
            }
            KeyCode::Enter => activate(app, index, selected),
            KeyCode::Char(c) => {
                if let Some(idx) = find_by_initial(items_of(index), selected, c) {
                    app.menu.focus = MenuFocus::Open {
                        index,
                        selected: idx,
                        scroll,
                    };
                    rescroll(app);
                }
            }
            _ => {}
        },
    }
}

/// Mouse-side activation, shared with the click handler so a clicked row and an
/// `Enter`-ed row go through exactly the same path.
pub(in crate::event) fn activate_item(app: &mut App, menu_idx: usize, item_idx: usize) {
    activate(app, menu_idx, item_idx);
}
