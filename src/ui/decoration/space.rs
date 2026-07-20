//! Space decoration mode ⭐🌙🪐🌠🚀 — twinkling stars, drifting planets, and
//! the occasional shooting star or rocket.

use ratatui::Frame;
use ratatui::layout::Rect;

use crate::theme::Theme;

use super::{DecorationActivity, pseudo_random, render_grid};

/// A twinkling star in the night sky.
#[derive(Debug, Clone)]
pub struct Star {
    pub x: u16,
    pub y: u16,
    /// Twinkling phase — the star is visible when this is above a threshold.
    pub phase: u16,
    pub emoji: &'static str,
}

/// A shooting star streaking across the sky.
#[derive(Debug, Clone)]
pub struct ShootingStar {
    pub x: f32,
    pub y: f32,
}

/// A planet drifting horizontally.
#[derive(Debug, Clone)]
pub struct Planet {
    pub x: f32,
    pub y: u16,
    pub speed: f32,
    pub direction: i8,
    pub emoji: &'static str,
}

/// Full space animation state.
#[derive(Debug, Clone, Default)]
pub struct SpaceState {
    pub stars: Vec<Star>,
    pub shooting_stars: Vec<ShootingStar>,
    pub planets: Vec<Planet>,
    pub initialized: bool,
}

const STAR_EMOJIS: &[&str] = &[
    "\u{2B50}", // ⭐
    "\u{2728}", // ✨
];
const SHOOTING_STAR: &str = "\u{1F320}"; // 🌠
const PLANET_EMOJIS: &[&str] = &[
    "\u{1FA90}", // 🪐
    "\u{1F319}", // 🌙
];
const ROCKET: &str = "\u{1F680}"; // 🚀

fn initialize_space(state: &mut SpaceState, width: u16, height: u16) {
    if width < 4 || height < 3 {
        state.initialized = true;
        return;
    }

    state.stars.clear();
    let star_count = (width as usize / 4).clamp(3, 10);
    for i in 0..star_count {
        let x = ((i as u64 * 7 + 3) % width as u64) as u16;
        let y = ((i as u64 * 5 + 1) % height.saturating_sub(1) as u64) as u16;
        let emoji = STAR_EMOJIS[i % STAR_EMOJIS.len()];
        state.stars.push(Star {
            x: x.min(width.saturating_sub(2)),
            y,
            phase: (i as u16 * 37) % 100,
            emoji,
        });
    }

    state.planets.clear();
    let planet_count = if width >= 10 { 2 } else { 1 };
    for i in 0..planet_count {
        let x = (width as f32) * (i as f32 + 1.0) / (planet_count as f32 + 1.0);
        let y = ((i as u16 + 1) * height / 3).min(height.saturating_sub(2));
        state.planets.push(Planet {
            x,
            y,
            speed: 0.2 + i as f32 * 0.1,
            direction: if i % 2 == 0 { 1 } else { -1 },
            emoji: PLANET_EMOJIS[i % PLANET_EMOJIS.len()],
        });
    }

    state.shooting_stars.clear();
    state.initialized = true;
}

/// Advance space animation by one tick.
pub(super) fn tick_space(
    state: &mut SpaceState,
    tick: u64,
    width: u16,
    height: u16,
    activity: DecorationActivity,
) {
    if width < 4 || height < 3 {
        return;
    }
    if !state.initialized {
        initialize_space(state, width, height);
    }

    // Twinkle stars — advance phase every tick.
    for (i, star) in state.stars.iter_mut().enumerate() {
        // Each star has a different twinkle speed derived from its index.
        let speed = 3 + (i as u16 % 5);
        star.phase = star.phase.wrapping_add(speed) % 100;
    }

    // Move planets slowly.
    if tick.is_multiple_of(5) {
        let max_x = width.saturating_sub(2) as f32;
        for planet in &mut state.planets {
            planet.x += planet.speed * planet.direction as f32;
            if planet.x < 0.0 {
                planet.x = 0.0;
                planet.direction = 1;
            } else if planet.x > max_x {
                planet.x = max_x;
                planet.direction = -1;
            }
        }
    }

    // Move shooting stars (fast diagonal — every tick).
    for ss in &mut state.shooting_stars {
        ss.x += 1.5;
        ss.y += 0.5;
    }
    state
        .shooting_stars
        .retain(|ss| (ss.x as u16) < width && (ss.y as u16) < height);

    // Spawn shooting stars based on activity.
    let (spawn_interval, max_shooting) = match activity {
        DecorationActivity::Calm => (25_u64, 1_usize),
        DecorationActivity::Active => (10, 3),
    };
    if tick.is_multiple_of(spawn_interval) && state.shooting_stars.len() < max_shooting {
        let x = 0.0_f32;
        let y = (pseudo_random(tick, 42) % height.saturating_sub(2) as u64) as f32;
        state.shooting_stars.push(ShootingStar { x, y });
    }

    // In Active mode, occasionally turn a shooting star into a rocket (reuse slot).
    // We represent rockets as shooting stars with a flag via the emoji chosen at render time.
}

/// Render the space scene.
pub(super) fn render_space(frame: &mut Frame, area: Rect, state: &SpaceState, theme: &Theme) {
    if area.width < 4 || area.height < 2 {
        return;
    }

    let w = area.width as usize;
    let h = area.height as usize;
    let mut grid: Vec<Vec<Option<&str>>> = vec![vec![None; w]; h];

    // Place stars (visible when phase > 40 — roughly 60% of the time).
    for star in &state.stars {
        if star.phase > 40 {
            let col = (star.x as usize).min(w.saturating_sub(2));
            let row = (star.y as usize).min(h.saturating_sub(1));
            if col + 1 < w {
                grid[row][col] = Some(star.emoji);
            }
        }
    }

    // Place planets.
    for planet in &state.planets {
        let col = (planet.x as usize).min(w.saturating_sub(2));
        let row = (planet.y as usize).min(h.saturating_sub(1));
        if col + 1 < w {
            grid[row][col] = Some(planet.emoji);
        }
    }

    // Place shooting stars / rockets.
    for (i, ss) in state.shooting_stars.iter().enumerate() {
        let col = (ss.x as usize).min(w.saturating_sub(2));
        let row = (ss.y as usize).min(h.saturating_sub(1));
        if col + 1 < w {
            // First shooting star in Active mode renders as a rocket.
            let emoji = if i == 0 { ROCKET } else { SHOOTING_STAR };
            grid[row][col] = Some(emoji);
        }
    }

    render_grid(frame, area, &grid, theme);
}
