//! Menu bar interaction state, plus the pure navigation helpers the keyboard
//! and mouse handlers share.
//!
//! The navigation helpers are free functions over `&[MenuItem]` rather than
//! methods on `App`, so the separator-skipping and wrap-around rules can be
//! unit-tested without standing up a terminal or an `App`.

use ratatui::layout::Rect;

use super::model::MenuItem;

/// Where menu-bar interaction currently sits.
///
/// Three states rather than two because `F10` focuses the bar *without*
/// committing to a menu — matching the GTK/Windows convention where the arrow
/// keys then browse the titles and `Down`/`Enter` drops the list open. Merging
/// `Bar` into `Open` would force `F10` to pop a dropdown the user never asked
/// for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MenuFocus {
    /// The menu bar is drawn but inert; keys go to the app as usual.
    #[default]
    Closed,
    /// The bar has keyboard focus and `index` is highlighted, no dropdown yet.
    Bar { index: usize },
    /// `index`'s dropdown is open with `selected` highlighted.
    Open {
        index: usize,
        selected: usize,
        scroll: usize,
    },
}

impl MenuFocus {
    /// Whether the menu is consuming input. When true the event dispatcher
    /// routes every key to the menu handler and nothing reaches the panels.
    pub fn is_active(self) -> bool {
        !matches!(self, MenuFocus::Closed)
    }

    /// The highlighted top-level menu, in either active state.
    pub fn active_index(self) -> Option<usize> {
        match self {
            MenuFocus::Closed => None,
            MenuFocus::Bar { index } => Some(index),
            MenuFocus::Open { index, .. } => Some(index),
        }
    }

    /// The menu whose dropdown is open, if any.
    pub fn open_index(self) -> Option<usize> {
        match self {
            MenuFocus::Open { index, .. } => Some(index),
            _ => None,
        }
    }
}

/// A clickable top-level title on the menu bar row: `x0` inclusive, `x1`
/// exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BarHit {
    pub x0: u16,
    pub x1: u16,
    /// Index into [`MENUS`](super::model::MENUS).
    pub menu: usize,
}

/// A clickable row inside the open dropdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemHit {
    /// Absolute screen row.
    pub y: u16,
    /// Index into the open menu's `items`.
    pub item: usize,
    /// False for a row whose command is currently unavailable — it is drawn
    /// greyed out and clicking it does nothing.
    pub enabled: bool,
}

/// Menu bar state carried on `App`.
#[derive(Default)]
pub struct MenuState {
    pub focus: MenuFocus,
    /// Top-level title hit regions, recorded by the last bar render.
    pub bar_hits: Vec<BarHit>,
    /// Dropdown row hit regions, recorded by the last dropdown render. Empty
    /// while no dropdown is open.
    pub item_hits: Vec<ItemHit>,
    /// The open dropdown's rect (including its border), for outside-click
    /// detection. Zero-sized while closed.
    pub dropdown_area: Rect,
    /// Top-level title under the mouse, for the hover highlight.
    pub hover: Option<usize>,
}

impl MenuState {
    /// Drop every recorded dropdown region. Called whenever the dropdown
    /// closes so a stale rect can't keep swallowing clicks.
    pub fn clear_dropdown_regions(&mut self) {
        self.item_hits.clear();
        self.dropdown_area = Rect::default();
    }

    /// Give the bar keyboard focus without opening anything.
    pub fn focus_bar(&mut self, index: usize) {
        self.focus = MenuFocus::Bar { index };
        self.clear_dropdown_regions();
    }

    /// Open `index`'s dropdown with the first selectable row highlighted.
    pub fn open(&mut self, index: usize, items: &[MenuItem]) {
        self.focus = MenuFocus::Open {
            index,
            selected: first_selectable(items),
            scroll: 0,
        };
        self.clear_dropdown_regions();
    }

    /// Leave the menu entirely and hand input back to the app.
    ///
    /// Always call this *before* running a command: several commands open an
    /// overlay of their own, and closing afterwards would tear that overlay's
    /// state back down.
    pub fn close(&mut self) {
        self.focus = MenuFocus::Closed;
        self.clear_dropdown_regions();
    }

    /// Nudge `scroll` so `selected` stays inside a window of `visible` rows.
    pub fn scroll_selection_into_view(&mut self, visible: usize) {
        let MenuFocus::Open {
            selected, scroll, ..
        } = &mut self.focus
        else {
            return;
        };
        if visible == 0 {
            return;
        }
        if *selected < *scroll {
            *scroll = *selected;
        } else if *selected >= *scroll + visible {
            *scroll = *selected + 1 - visible;
        }
    }

    /// Which top-level title (if any) sits under `col` on the bar row.
    pub fn bar_hit_at(&self, col: u16) -> Option<usize> {
        self.bar_hits
            .iter()
            .find(|h| col >= h.x0 && col < h.x1)
            .map(|h| h.menu)
    }

    /// Which dropdown row (if any) sits at absolute screen row `row`, and
    /// whether it is enabled.
    pub fn item_hit_at(&self, row: u16) -> Option<ItemHit> {
        self.item_hits.iter().find(|h| h.y == row).copied()
    }

    /// Whether `(col, row)` lands inside the open dropdown's rect.
    pub fn in_dropdown(&self, col: u16, row: u16) -> bool {
        let a = self.dropdown_area;
        a.width > 0
            && a.height > 0
            && col >= a.x
            && col < a.x + a.width
            && row >= a.y
            && row < a.y + a.height
    }
}

// ── Pure navigation helpers ────────────────────────────────────────────────

/// The first selectable row in `items`, or 0 if there is none.
///
/// A menu whose rows are all separators is a table authoring mistake rather
/// than a runtime condition, so this degrades to 0 instead of returning an
/// `Option` the callers would all have to unwrap.
pub fn first_selectable(items: &[MenuItem]) -> usize {
    items.iter().position(MenuItem::is_selectable).unwrap_or(0)
}

/// The last selectable row in `items`, or 0 if there is none.
pub fn last_selectable(items: &[MenuItem]) -> usize {
    items
        .iter()
        .rposition(MenuItem::is_selectable)
        .unwrap_or(0)
}

/// Step the selection from `from` by one row in `dir` (`+1` down, `-1` up),
/// skipping separators and wrapping around the ends.
///
/// Disabled rows are deliberately still selectable: greying a row out signals
/// "not available right now" and skipping it would hide the row's existence,
/// which is the opposite of what the disabled state is for.
pub fn step_selection(items: &[MenuItem], from: usize, dir: i32) -> usize {
    let n = items.len();
    if n == 0 {
        return 0;
    }
    let mut idx = from.min(n - 1);
    // At most `n` steps: enough to land on any row, and to give up (returning
    // `from`) when the menu holds no selectable row at all.
    for _ in 0..n {
        idx = if dir >= 0 {
            (idx + 1) % n
        } else {
            (idx + n - 1) % n
        };
        if items[idx].is_selectable() {
            return idx;
        }
    }
    from
}

/// The next row whose label starts with `ch` (case-insensitive), searching
/// forward from `from` and wrapping — the type-ahead that lets `n` jump between
/// the "New …" entries of a menu.
pub fn find_by_initial(items: &[MenuItem], from: usize, ch: char) -> Option<usize> {
    let n = items.len();
    if n == 0 {
        return None;
    }
    let target = ch.to_ascii_lowercase();
    (1..=n)
        .map(|off| (from + off) % n)
        .find(|&idx| match &items[idx] {
            MenuItem::Command { label, .. } => label
                .chars()
                .next()
                .is_some_and(|c| c.to_ascii_lowercase() == target),
            MenuItem::Separator => false,
        })
}

/// Step the highlighted top-level menu by one, wrapping.
pub fn step_menu(menu_count: usize, from: usize, dir: i32) -> usize {
    if menu_count == 0 {
        return 0;
    }
    if dir >= 0 {
        (from + 1) % menu_count
    } else {
        (from + menu_count - 1) % menu_count
    }
}
