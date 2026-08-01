//! Menu bar rendering: the always-present strip of titles under the title bar,
//! and the dropdown of the open menu.
//!
//! Both passes record their hit regions onto `app.menu` so the mouse handler
//! resolves clicks against exactly what was drawn, the same contract
//! [`crate::ui::worktree_bar`] uses for the worktree strip.
//!
//! Styling follows the existing popups (`Clear` + `Borders::ALL` + an accent
//! border, `selected_bg`/`selected_fg` for the highlight) so the dropdown reads
//! as part of the same app rather than a bolted-on widget.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::menu::model::{MENUS, MenuItem};
use crate::menu::state::{BarHit, ItemHit};

/// Blank columns on each side of a top-level title, giving the highlight a bit
/// of breathing room and widening the click target.
const TITLE_PAD: u16 = 1;

/// Columns between the end of a row's label and the start of its shortcut.
const LABEL_CHORD_GAP: u16 = 4;

fn width(s: &str) -> u16 {
    UnicodeWidthStr::width(s) as u16
}

/// Draw the menu bar row and record the click regions of each title.
pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.height == 0 || area.width == 0 {
        app.menu.bar_hits.clear();
        return;
    }

    let theme = &app.theme;
    let active = app.menu.focus.active_index();
    let hover = app.menu.hover;

    // Color::Reset so the terminal's own background (including a background
    // image) shows through, matching the title bar above it.
    let bar_bg = ratatui::style::Color::Reset;

    let mut spans: Vec<Span> = Vec::new();
    let mut hits: Vec<BarHit> = Vec::new();
    let mut x = area.x;

    for (i, menu) in MENUS.iter().enumerate() {
        let text = format!(
            "{pad}{title}{pad}",
            pad = " ".repeat(TITLE_PAD as usize),
            title = menu.title
        );
        let w = width(&text);

        // Stop before overflowing the row; a clipped, half-drawn title would
        // record a hit region that doesn't match what's on screen.
        if x + w > area.x + area.width {
            break;
        }

        let style = if active == Some(i) {
            // The open/focused menu is a *selection*, so it gets the same
            // background treatment as any other selected row in the app.
            Style::default()
                .fg(theme.selected_fg)
                .bg(theme.selected_bg)
                .add_modifier(Modifier::BOLD)
        } else if hover == Some(i) {
            // Hover is expressed in the foreground only: several themes have a
            // background-image-friendly transparent bar, and painting a hover
            // background there fights the title bar's `Color::Reset`.
            Style::default()
                .fg(theme.accent)
                .bg(bar_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg).bg(bar_bg)
        };

        spans.push(Span::styled(text, style));
        hits.push(BarHit {
            x0: x,
            x1: x + w,
            menu: i,
        });
        x += w;
    }

    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(bar_bg)),
        area,
    );
    app.menu.bar_hits = hits;
}

/// Draw the open menu's dropdown, if any, and record its row hit regions.
///
/// Called after the panels so the popup lands on top of them. `frame_area` is
/// the whole screen: the dropdown is clamped to it rather than to the main
/// area, since it hangs off the menu bar which sits above the main area.
pub fn render_dropdown(frame: &mut Frame, frame_area: Rect, app: &mut App) {
    let (menu_idx, selected, scroll) = match app.menu.focus {
        crate::menu::MenuFocus::Open {
            index,
            selected,
            scroll,
        } => (index, selected, scroll),
        _ => {
            app.menu.clear_dropdown_regions();
            return;
        }
    };
    let Some(menu) = MENUS.get(menu_idx) else {
        app.menu.clear_dropdown_regions();
        return;
    };

    // Shortcut hints are resolved for the focused panel's layer, so a row shows
    // the chord that would actually fire right now — the same rule the command
    // palette uses.
    let context = app.focus.key_context();
    let rows: Vec<Row> = menu
        .items
        .iter()
        .map(|item| match item {
            MenuItem::Separator => Row::Separator,
            MenuItem::Command { id, label } => {
                let chord = crate::command_palette::COMMANDS
                    .iter()
                    .find(|c| c.id == *id)
                    .and_then(|c| c.action)
                    .and_then(|a| {
                        crate::ui::common::representative_chord(&app.keymap, context, a)
                    })
                    .unwrap_or_default();
                Row::Command {
                    label,
                    chord,
                    enabled: crate::menu::command_enabled(*id, app),
                }
            }
        })
        .collect();

    // ── Geometry ──────────────────────────────────────────────────────────
    let label_w = rows
        .iter()
        .map(|r| match r {
            Row::Command { label, .. } => width(label),
            Row::Separator => 0,
        })
        .max()
        .unwrap_or(0);
    let chord_w = rows
        .iter()
        .map(|r| match r {
            Row::Command { chord, .. } => width(chord),
            Row::Separator => 0,
        })
        .max()
        .unwrap_or(0);

    // 2 border columns + a leading and trailing pad column.
    let content_w = label_w + LABEL_CHORD_GAP + chord_w;
    let popup_w = (content_w + 4).min(frame_area.width);

    let anchor_x = app
        .menu
        .bar_hits
        .iter()
        .find(|h| h.menu == menu_idx)
        .map(|h| h.x0)
        .unwrap_or(frame_area.x);
    // Keep the popup on screen when a right-hand menu would overhang.
    let max_x = (frame_area.x + frame_area.width).saturating_sub(popup_w);
    let popup_x = anchor_x.min(max_x);

    let popup_y = app.layout.cache.menubar_area.y + app.layout.cache.menubar_area.height;
    let avail_h = (frame_area.y + frame_area.height).saturating_sub(popup_y);
    // 2 rows of border; at least one content row or there is nothing to show.
    let popup_h = ((rows.len() as u16) + 2).min(avail_h);
    if popup_h < 3 || popup_w < 4 {
        app.menu.clear_dropdown_regions();
        return;
    }
    let popup_area = Rect::new(popup_x, popup_y, popup_w, popup_h);

    frame.render_widget(Clear, popup_area);
    let block = Block::default()
        .title(format!(" {} ", menu.title))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.accent));
    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    // ── Rows ──────────────────────────────────────────────────────────────
    let visible = inner.height as usize;
    let start = scroll.min(rows.len().saturating_sub(visible.max(1)));
    let theme = &app.theme;
    let mut lines: Vec<Line> = Vec::new();
    let mut hits: Vec<ItemHit> = Vec::new();

    for (offset, row) in rows.iter().skip(start).take(visible).enumerate() {
        let y = inner.y + offset as u16;
        let idx = start + offset;
        match row {
            Row::Separator => {
                lines.push(Line::from(Span::styled(
                    "─".repeat(inner.width as usize),
                    Style::default().fg(theme.border_unfocused),
                )));
            }
            Row::Command {
                label,
                chord,
                enabled,
            } => {
                let is_selected = idx == selected;
                let pad = (inner.width as usize)
                    .saturating_sub(width(label) as usize + width(chord) as usize + 2);
                // A disabled row keeps its place and its label but loses the
                // shortcut hint: showing a chord that currently does nothing
                // would be a lie about what the key does.
                let shown_chord = if *enabled { chord.as_str() } else { "" };
                let pad = if *enabled {
                    pad
                } else {
                    pad + width(chord) as usize
                };

                let (label_style, chord_style) = match (is_selected, *enabled) {
                    (true, true) => (
                        Style::default()
                            .fg(theme.selected_fg)
                            .bg(theme.selected_bg)
                            .add_modifier(Modifier::BOLD),
                        Style::default().fg(theme.selected_fg).bg(theme.selected_bg),
                    ),
                    // A selected-but-disabled row still shows where the cursor
                    // is, so arrowing through the menu never appears to stall.
                    (true, false) => (
                        Style::default()
                            .fg(theme.selected_fg)
                            .bg(theme.selected_bg)
                            .add_modifier(Modifier::DIM),
                        Style::default().fg(theme.selected_fg).bg(theme.selected_bg),
                    ),
                    (false, true) => (
                        Style::default().fg(theme.fg),
                        Style::default().fg(theme.hint),
                    ),
                    // DIM over the normal foreground rather than `theme.muted`:
                    // muted is at or near the background in several of the
                    // bundled themes, which would make the row vanish instead
                    // of reading as unavailable.
                    (false, false) => (
                        Style::default().fg(theme.fg).add_modifier(Modifier::DIM),
                        Style::default().fg(theme.fg).add_modifier(Modifier::DIM),
                    ),
                };

                lines.push(Line::from(vec![
                    Span::styled(format!(" {label}"), label_style),
                    Span::styled(" ".repeat(pad), label_style),
                    Span::styled(format!("{shown_chord} "), chord_style),
                ]));
                hits.push(ItemHit {
                    y,
                    item: idx,
                    enabled: *enabled,
                });
            }
        }
    }

    frame.render_widget(Paragraph::new(lines), inner);
    app.menu.item_hits = hits;
    app.menu.dropdown_area = popup_area;
}

/// A dropdown row resolved for rendering: label, live shortcut, availability.
enum Row {
    Command {
        label: &'static str,
        chord: String,
        enabled: bool,
    },
    Separator,
}

/// How many content rows the dropdown can show at `frame_height`, used by the
/// key handler to keep the selection scrolled into view. Mirrors the clamp in
/// [`render_dropdown`].
pub fn visible_rows(app: &App, frame_height: u16) -> usize {
    let popup_y = app.layout.cache.menubar_area.y + app.layout.cache.menubar_area.height;
    let avail_h = frame_height.saturating_sub(popup_y);
    avail_h.saturating_sub(2) as usize
}
