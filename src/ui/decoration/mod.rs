//! Decoration rendering for the worktree panel's empty space.
//!
//! Supports multiple animated modes: aquarium, space, garden, city.
//! Each mode has its own state struct, `tick_*` (animation update), and
//! `render_*` (drawing) function, split into one submodule per mode.
//! The top-level [`tick_decoration`] and [`render_decoration`] dispatch to
//! the active mode.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::theme::Theme;

mod aquarium;
mod city;
mod garden;
mod space;

#[cfg(test)]
mod tests;

// These per-mode types are only ever reached through their `*State` struct
// fields (`DecorationStates::aquarium.fish`, etc.), never named directly
// outside this module — same as before the split, when they were `pub`
// struct definitions living directly in this file.
#[allow(unused_imports)]
pub use aquarium::{AquariumState, Bubble, Fish};
#[allow(unused_imports)]
pub use city::{Building, Car, CityState, SkyObject};
#[allow(unused_imports)]
pub use garden::{Bird, Butterfly, GardenPlant, GardenState};
#[allow(unused_imports)]
pub use space::{Planet, ShootingStar, SpaceState, Star};

use aquarium::{render_aquarium, tick_aquarium};
use city::{render_city, tick_city};
use garden::{render_garden, tick_garden};
use space::{render_space, tick_space};

// ═══════════════════════════════════════════════════════════════════════
// Shared types
// ═══════════════════════════════════════════════════════════════════════

/// Decoration mode parsed from config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecorationMode {
    Aquarium,
    Space,
    Garden,
    City,
    None,
}

impl DecorationMode {
    pub fn from_str(s: &str) -> Self {
        match s {
            "none" => Self::None,
            "space" => Self::Space,
            "garden" => Self::Garden,
            "city" => Self::City,
            _ => Self::Aquarium,
        }
    }

    /// Returns `true` when the mode runs an animation that needs periodic ticks.
    pub fn has_animation(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// Activity level that affects animation intensity across all modes.
///
/// `Active` — Claude Code is waiting for user input (more lively).
/// `Calm`   — Claude Code is busy working (relaxed animation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecorationActivity {
    Calm,
    Active,
}

/// Container holding state for every decoration mode.
///
/// Only the state corresponding to the active mode is actually used;
/// the others stay at their default (uninitialised) values until the
/// user switches modes.
#[derive(Debug, Clone, Default)]
pub struct DecorationStates {
    pub aquarium: AquariumState,
    pub space: SpaceState,
    pub garden: GardenState,
    pub city: CityState,
}

/// Advance the active decoration by one tick.
pub fn tick_decoration(
    states: &mut DecorationStates,
    tick: u64,
    width: u16,
    height: u16,
    activity: DecorationActivity,
    mode: DecorationMode,
) {
    match mode {
        DecorationMode::Aquarium => {
            tick_aquarium(&mut states.aquarium, tick, width, height, activity);
        }
        DecorationMode::Space => {
            tick_space(&mut states.space, tick, width, height, activity);
        }
        DecorationMode::Garden => {
            tick_garden(&mut states.garden, tick, width, height, activity);
        }
        DecorationMode::City => {
            tick_city(&mut states.city, tick, width, height, activity);
        }
        DecorationMode::None => {}
    }
}

/// Dispatch decoration rendering based on config mode.
pub fn render_decoration(
    frame: &mut Frame,
    area: Rect,
    states: &DecorationStates,
    theme: &Theme,
    mode: DecorationMode,
) {
    match mode {
        DecorationMode::Aquarium => {
            let block = Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme.border_unfocused));
            let inner = block.inner(area);
            frame.render_widget(block, area);
            render_aquarium(frame, inner, &states.aquarium, theme);
        }
        DecorationMode::Space => {
            let block = Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme.border_unfocused));
            let inner = block.inner(area);
            frame.render_widget(block, area);
            render_space(frame, inner, &states.space, theme);
        }
        DecorationMode::Garden => {
            let block = Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme.border_unfocused));
            let inner = block.inner(area);
            frame.render_widget(block, area);
            render_garden(frame, inner, &states.garden, theme);
        }
        DecorationMode::City => {
            let block = Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme.border_unfocused));
            let inner = block.inner(area);
            frame.render_widget(block, area);
            render_city(frame, inner, &states.city, theme);
        }
        DecorationMode::None => {}
    }
}

/// Build a row-major grid of emoji cells and render it as a [`Paragraph`].
///
/// This helper is shared by all modes.  Each grid cell is either `None`
/// (rendered as a space) or `Some(emoji)` (rendered as a 2-column-wide
/// styled span).
fn render_grid(frame: &mut Frame, area: Rect, grid: &[Vec<Option<&str>>], theme: &Theme) {
    let lines: Vec<Line> = grid
        .iter()
        .map(|row| {
            let mut spans: Vec<Span> = Vec::new();
            let mut col = 0;
            while col < row.len() {
                if let Some(emoji) = row[col] {
                    spans.push(Span::styled(emoji, Style::default().fg(theme.fg)));
                    col += 2; // emoji is 2 cells wide
                } else {
                    spans.push(Span::raw(" "));
                    col += 1;
                }
            }
            Line::from(spans)
        })
        .collect();

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, area);
}

/// Simple pseudo-random number derived from the tick counter.
///
/// Not cryptographically secure — just good enough for animation variety.
fn pseudo_random(tick: u64, seed: u64) -> u64 {
    let mut x = tick.wrapping_mul(6364136223846793005).wrapping_add(seed);
    x ^= x >> 16;
    x = x.wrapping_mul(2246822519);
    x ^= x >> 13;
    x
}
