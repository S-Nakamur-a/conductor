//! Worktree panel — left-most column showing the worktree list.
//!
//! Displays the list of worktrees with selection, status indicators,
//! detail info, and an optional decoration zone (aquarium).
//!
//! Split by rendering responsibility: [`list`] draws the worktree/session
//! list (zone 1), [`detail`] the selected worktree's detail section
//! (zone 2). Zone 3 (decoration) is rendered directly via
//! [`crate::ui::decoration`].

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};

use crate::app::{App, Focus};
use crate::ui::decoration::{self, DecorationMode};

mod detail;
mod list;
#[cfg(test)]
mod tests;

/// Render the worktree panel into the given area.
pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let focused = app.focus == Focus::Worktree;
    let border_color = if focused {
        app.theme.border_focused
    } else {
        app.theme.border_unfocused
    };

    // ── Zone layout calculation ────────────────────────────────────
    // Zone 1: worktree + session list  — 40% (or more)
    // Zone 2: detail section           — 60% (or less)
    // Zone 3: decoration (optional)    — 20%
    let decoration_mode = DecorationMode::from_str(&app.config.general.decoration);

    let zones = if area.height < 10 {
        // Too small: only show the list.
        Layout::vertical([
            Constraint::Percentage(100),
            Constraint::Length(0),
            Constraint::Length(0),
        ])
        .split(area)
    } else if decoration_mode != DecorationMode::None {
        // Decoration enabled: 3-zone layout.
        Layout::vertical([
            Constraint::Percentage(40),
            Constraint::Percentage(40),
            Constraint::Percentage(20),
        ])
        .split(area)
    } else {
        // No decoration: 2-zone layout.
        Layout::vertical([
            Constraint::Percentage(40),
            Constraint::Percentage(60),
            Constraint::Length(0),
        ])
        .split(area)
    };

    // ── Zone 1: Worktree list ─────────────────────────────────────
    list::render_worktree_list(frame, zones[0], app, focused, border_color);

    // ── Zone 2: Detail section ────────────────────────────────────
    if zones[1].height >= 3 {
        let theme = &app.theme;
        detail::render_detail(frame, zones[1], app, theme, border_color);
    }

    // ── Zone 3: Decoration (optional) ────────────────────────────
    if zones[2].height >= 4 {
        decoration::render_decoration(
            frame,
            zones[2],
            &app.decoration_states,
            &app.theme,
            decoration_mode,
        );
    }
}
