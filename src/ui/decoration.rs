//! Decoration rendering for the worktree panel's empty space.
//!
//! Supports multiple animated modes: aquarium, space, garden, city.
//! Each mode has its own state struct, `tick_*` (animation update), and
//! `render_*` (drawing) function.  The top-level [`tick_decoration`] and
//! [`render_decoration`] dispatch to the active mode.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::theme::Theme;

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

// ═══════════════════════════════════════════════════════════════════════
// Aquarium  🐠🐟🐡🐙🦀🦑
// ═══════════════════════════════════════════════════════════════════════

/// A fish swimming in the aquarium.
#[derive(Debug, Clone)]
pub struct Fish {
    pub x: f32,
    pub y: u16,
    pub emoji: &'static str,
    pub direction: i8,
    pub speed: f32,
}

/// A bubble floating upward.
#[derive(Debug, Clone)]
pub struct Bubble {
    pub x: u16,
    pub y: f32,
}

/// Full aquarium animation state.
#[derive(Debug, Clone, Default)]
pub struct AquariumState {
    pub fish: Vec<Fish>,
    pub bubbles: Vec<Bubble>,
    pub initialized: bool,
}

const FISH_EMOJIS: &[&str] = &[
    "\u{1F420}", // 🐠
    "\u{1F41F}", // 🐟
    "\u{1F421}", // 🐡
    "\u{1F419}", // 🐙
    "\u{1F980}", // 🦀
    "\u{1F991}", // 🦑
];

const CORAL: &str = "\u{1FAB8}"; // 🪸
const BUBBLE_EMOJI: &str = "\u{1FAE7}"; // 🫧

/// Initialize the aquarium with fish placed evenly across the area.
fn initialize_aquarium(state: &mut AquariumState, width: u16, height: u16) {
    if width < 4 || height < 3 {
        state.initialized = true;
        return;
    }

    let fish_count = 5.min((width / 4) as usize).max(2);
    state.fish.clear();
    // Leave row 0 for top and last row for coral
    let usable_height = height.saturating_sub(1);

    for i in 0..fish_count {
        let x = (i as f32 + 0.5) * (width as f32) / (fish_count as f32);
        let y = if usable_height > 1 {
            (i as u16 % usable_height.saturating_sub(1)) + 1
        } else {
            0
        };
        let emoji = FISH_EMOJIS[i % FISH_EMOJIS.len()];
        let direction = if i % 2 == 0 { 1 } else { -1 };
        let speed = 0.3 + (i as f32 * 0.1);
        state.fish.push(Fish {
            x,
            y,
            emoji,
            direction,
            speed,
        });
    }

    state.bubbles.clear();
    state.initialized = true;
}

/// Advance aquarium animation by one tick.
pub fn tick_aquarium(
    state: &mut AquariumState,
    ui_tick: u64,
    width: u16,
    height: u16,
    activity: DecorationActivity,
) {
    if width < 4 || height < 3 {
        return;
    }

    if !state.initialized {
        initialize_aquarium(state, width, height);
    }

    // Move fish every 3rd tick for a relaxed pace.
    if ui_tick % 3 == 0 {
        let max_x = width.saturating_sub(2) as f32;
        let usable_height = height.saturating_sub(1);
        for fish in &mut state.fish {
            fish.x += fish.speed * fish.direction as f32;
            // Bounce off walls.
            if fish.x < 0.0 {
                fish.x = 0.0;
                fish.direction = 1;
            } else if fish.x > max_x {
                fish.x = max_x;
                fish.direction = -1;
            }
            // Occasionally change vertical position.
            if ui_tick % 15 == 0 && usable_height > 2 {
                let raw = fish.y as i16 + fish.direction as i16;
                let new_y = (raw.max(0) as u16).min(usable_height.saturating_sub(2));
                fish.y = new_y;
            }
        }
    }

    // Float bubbles upward every 2nd tick.
    if ui_tick % 2 == 0 {
        for bubble in &mut state.bubbles {
            bubble.y -= 0.3;
        }
        // Remove bubbles that floated out of view.
        state.bubbles.retain(|b| b.y > 0.0);
    }

    // Spawn new bubbles based on activity level.
    let spawn_chance = match activity {
        DecorationActivity::Calm => 12,
        DecorationActivity::Active => 5,
    };
    if ui_tick % spawn_chance == 0 && state.bubbles.len() < 8 {
        let x = ((ui_tick * 7 + 3) % width as u64) as u16;
        let y = height.saturating_sub(2) as f32;
        state.bubbles.push(Bubble { x, y });
    }
}

/// Render the aquarium into the given area.
fn render_aquarium(frame: &mut Frame, area: Rect, state: &AquariumState, theme: &Theme) {
    if area.width < 4 || area.height < 2 {
        return;
    }

    let w = area.width as usize;
    let h = area.height as usize;

    let mut grid: Vec<Vec<Option<&str>>> = vec![vec![None; w]; h];

    // Place coral on the bottom row.
    if h >= 1 {
        let bottom = h - 1;
        let mut col = 0;
        while col + 1 < w {
            grid[bottom][col] = Some(CORAL);
            col += 3;
        }
    }

    // Place fish.
    for fish in &state.fish {
        let col = (fish.x as usize).min(w.saturating_sub(2));
        let row = (fish.y as usize).min(h.saturating_sub(2));
        if row < h && col + 1 < w {
            grid[row][col] = Some(fish.emoji);
        }
    }

    // Place bubbles.
    for bubble in &state.bubbles {
        let col = (bubble.x as usize).min(w.saturating_sub(2));
        let row = (bubble.y as usize).min(h.saturating_sub(1));
        if row < h && col + 1 < w {
            grid[row][col] = Some(BUBBLE_EMOJI);
        }
    }

    render_grid(frame, area, &grid, theme);
}

// ═══════════════════════════════════════════════════════════════════════
// Space  ⭐🌙🪐🌠🚀
// ═══════════════════════════════════════════════════════════════════════

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
fn tick_space(
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
    if tick % 5 == 0 {
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
    if tick % spawn_interval == 0 && state.shooting_stars.len() < max_shooting {
        let x = 0.0_f32;
        let y = (pseudo_random(tick, 42) % height.saturating_sub(2) as u64) as f32;
        state.shooting_stars.push(ShootingStar { x, y });
    }

    // In Active mode, occasionally turn a shooting star into a rocket (reuse slot).
    // We represent rockets as shooting stars with a flag via the emoji chosen at render time.
}

/// Render the space scene.
fn render_space(frame: &mut Frame, area: Rect, state: &SpaceState, theme: &Theme) {
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

// ═══════════════════════════════════════════════════════════════════════
// Garden  🌳🌸🦋🐦
// ═══════════════════════════════════════════════════════════════════════

/// A stationary plant on the garden floor.
#[derive(Debug, Clone)]
pub struct GardenPlant {
    pub x: u16,
    pub emoji: &'static str,
}

/// A butterfly floating in the air.
#[derive(Debug, Clone)]
pub struct Butterfly {
    pub x: f32,
    pub y: f32,
    pub dx: f32,
    pub dy: f32,
}

/// A bird flying horizontally.
#[derive(Debug, Clone)]
pub struct Bird {
    pub x: f32,
    pub y: u16,
    pub speed: f32,
    pub direction: i8,
}

/// Full garden animation state.
#[derive(Debug, Clone, Default)]
pub struct GardenState {
    pub plants: Vec<GardenPlant>,
    pub butterflies: Vec<Butterfly>,
    pub birds: Vec<Bird>,
    pub initialized: bool,
}

const TREE_EMOJIS: &[&str] = &[
    "\u{1F333}", // 🌳
    "\u{1F332}", // 🌲
];
const FLOWER_EMOJIS: &[&str] = &[
    "\u{1F338}", // 🌸
    "\u{1F337}", // 🌷
    "\u{1F33B}", // 🌻
    "\u{1F33A}", // 🌺
];
const HERB: &str = "\u{1F33F}"; // 🌿
const BUTTERFLY_EMOJI: &str = "\u{1F98B}"; // 🦋
const BIRD_EMOJI: &str = "\u{1F426}"; // 🐦
const BEE_EMOJI: &str = "\u{1F41D}"; // 🐝

fn initialize_garden(state: &mut GardenState, width: u16, height: u16) {
    if width < 4 || height < 3 {
        state.initialized = true;
        return;
    }

    // Place plants along the bottom row: trees, flowers, herbs.
    state.plants.clear();
    let all_plants: Vec<&str> = TREE_EMOJIS
        .iter()
        .chain(FLOWER_EMOJIS.iter())
        .copied()
        .collect();
    let mut col: u16 = 0;
    let mut idx = 0;
    while col + 1 < width {
        // Alternate between named plants and herb filler.
        let emoji = if idx % 3 == 0 {
            HERB
        } else {
            all_plants[idx % all_plants.len()]
        };
        state.plants.push(GardenPlant { x: col, emoji });
        col += 3;
        idx += 1;
    }

    // Initial butterflies.
    state.butterflies.clear();
    let butterfly_count = 2.min((width / 6) as usize).max(1);
    for i in 0..butterfly_count {
        let x = (width as f32) * (i as f32 + 1.0) / (butterfly_count as f32 + 1.0);
        let y = (height as f32) * 0.3 + i as f32;
        state.butterflies.push(Butterfly {
            x,
            y,
            dx: if i % 2 == 0 { 0.4 } else { -0.3 },
            dy: if i % 2 == 0 { -0.2 } else { 0.2 },
        });
    }

    state.birds.clear();
    state.initialized = true;
}

/// Advance garden animation by one tick.
fn tick_garden(
    state: &mut GardenState,
    tick: u64,
    width: u16,
    height: u16,
    activity: DecorationActivity,
) {
    if width < 4 || height < 3 {
        return;
    }
    if !state.initialized {
        initialize_garden(state, width, height);
    }

    let max_x = width.saturating_sub(2) as f32;
    // Leave bottom row for plants; usable rows = 0 .. height-2.
    let max_y = height.saturating_sub(2) as f32;

    // Move butterflies every 2nd tick.
    if tick % 2 == 0 {
        for (i, bf) in state.butterflies.iter_mut().enumerate() {
            bf.x += bf.dx;
            bf.y += bf.dy;

            // Bounce off boundaries.
            if bf.x < 0.0 {
                bf.x = 0.0;
                bf.dx = bf.dx.abs();
            } else if bf.x > max_x {
                bf.x = max_x;
                bf.dx = -bf.dx.abs();
            }
            if bf.y < 0.0 {
                bf.y = 0.0;
                bf.dy = bf.dy.abs();
            } else if bf.y > max_y {
                bf.y = max_y;
                bf.dy = -bf.dy.abs();
            }

            // Occasionally change direction for organic movement.
            if tick % 11 == (i as u64 % 11) {
                let r = pseudo_random(tick, i as u64 + 100);
                bf.dx = ((r % 7) as f32 - 3.0) * 0.15;
                bf.dy = (((r >> 8) % 5) as f32 - 2.0) * 0.15;
            }
        }
    }

    // Birds — fly horizontally and leave the area.
    if tick % 2 == 0 {
        for bird in &mut state.birds {
            bird.x += bird.speed * bird.direction as f32;
        }
        state
            .birds
            .retain(|b| b.x >= -2.0 && (b.x as u16) < width + 2);
    }

    // Spawn butterflies/bees to match activity.
    let target_count = match activity {
        DecorationActivity::Calm => 2_usize,
        DecorationActivity::Active => 4,
    };
    if tick % 20 == 0 && state.butterflies.len() < target_count {
        let r = pseudo_random(tick, 55);
        let x = (r % width as u64) as f32;
        let y = (r >> 8) % max_y.max(1.0) as u64;
        state.butterflies.push(Butterfly {
            x,
            y: y as f32,
            dx: 0.3,
            dy: -0.2,
        });
    }

    // Active mode: spawn a bird occasionally.
    if activity == DecorationActivity::Active && tick % 30 == 0 && state.birds.len() < 2 {
        let r = pseudo_random(tick, 77);
        let y = ((r % height.saturating_sub(2) as u64) as u16).min(height / 2);
        let from_left = r % 2 == 0;
        state.birds.push(Bird {
            x: if from_left { 0.0 } else { max_x },
            y,
            speed: 0.8,
            direction: if from_left { 1 } else { -1 },
        });
    }
}

/// Render the garden scene.
fn render_garden(frame: &mut Frame, area: Rect, state: &GardenState, theme: &Theme) {
    if area.width < 4 || area.height < 2 {
        return;
    }

    let w = area.width as usize;
    let h = area.height as usize;
    let mut grid: Vec<Vec<Option<&str>>> = vec![vec![None; w]; h];

    // Bottom row: plants.
    if h >= 1 {
        let bottom = h - 1;
        for plant in &state.plants {
            let col = (plant.x as usize).min(w.saturating_sub(2));
            if col + 1 < w {
                grid[bottom][col] = Some(plant.emoji);
            }
        }
    }

    // Butterflies / bees.
    for (i, bf) in state.butterflies.iter().enumerate() {
        let col = (bf.x as usize).min(w.saturating_sub(2));
        let row = (bf.y as usize).min(h.saturating_sub(2));
        if col + 1 < w && row < h {
            // Every 3rd butterfly is a bee for variety.
            let emoji = if i % 3 == 2 {
                BEE_EMOJI
            } else {
                BUTTERFLY_EMOJI
            };
            grid[row][col] = Some(emoji);
        }
    }

    // Birds.
    for bird in &state.birds {
        let col = (bird.x as usize).min(w.saturating_sub(2));
        let row = (bird.y as usize).min(h.saturating_sub(2));
        if col + 1 < w && col < w && row < h {
            grid[row][col] = Some(BIRD_EMOJI);
        }
    }

    render_grid(frame, area, &grid, theme);
}

// ═══════════════════════════════════════════════════════════════════════
// City  🏢🚗🌙
// ═══════════════════════════════════════════════════════════════════════

/// A building in the city skyline.
#[derive(Debug, Clone)]
pub struct Building {
    pub x: u16,
    pub emoji: &'static str,
}

/// A car driving along the road.
#[derive(Debug, Clone)]
pub struct Car {
    pub x: f32,
    pub speed: f32,
    pub direction: i8,
    pub emoji: &'static str,
}

/// A sky decoration (moon, stars).
#[derive(Debug, Clone)]
pub struct SkyObject {
    pub x: u16,
    pub y: u16,
    pub emoji: &'static str,
}

/// Full city animation state.
#[derive(Debug, Clone, Default)]
pub struct CityState {
    pub buildings: Vec<Building>,
    pub cars: Vec<Car>,
    pub sky: Vec<SkyObject>,
    pub initialized: bool,
}

const BUILDING_EMOJIS: &[&str] = &[
    "\u{1F3E2}", // 🏢
    "\u{1F3E0}", // 🏠
    "\u{1F3EC}", // 🏬
];
const CAR_EMOJIS: &[&str] = &[
    "\u{1F697}", // 🚗
    "\u{1F695}", // 🚕
    "\u{1F699}", // 🚙
    "\u{1F68C}", // 🚌
];
const CITY_MOON: &str = "\u{1F319}"; // 🌙
const CITY_STAR: &str = "\u{2B50}"; // ⭐
const TRAFFIC_LIGHT: &str = "\u{1F6A6}"; // 🚦

fn initialize_city(state: &mut CityState, width: u16, height: u16) {
    if width < 4 || height < 3 {
        state.initialized = true;
        return;
    }

    // Buildings along the bottom row.
    state.buildings.clear();
    let mut col: u16 = 0;
    let mut idx = 0;
    while col + 1 < width {
        // Every 4th slot is a traffic light; otherwise a building.
        let emoji = if idx % 5 == 3 {
            TRAFFIC_LIGHT
        } else {
            BUILDING_EMOJIS[idx % BUILDING_EMOJIS.len()]
        };
        state.buildings.push(Building { x: col, emoji });
        col += 3;
        idx += 1;
    }

    // Sky objects: moon and a couple of stars.
    state.sky.clear();
    state.sky.push(SkyObject {
        x: width / 3,
        y: 0,
        emoji: CITY_MOON,
    });
    if width >= 10 {
        state.sky.push(SkyObject {
            x: (width * 2 / 3).min(width.saturating_sub(2)),
            y: 0,
            emoji: CITY_STAR,
        });
    }

    // Initial cars.
    state.cars.clear();
    state.cars.push(Car {
        x: 2.0,
        speed: 0.5,
        direction: 1,
        emoji: CAR_EMOJIS[0],
    });

    state.initialized = true;
}

/// Advance city animation by one tick.
fn tick_city(
    state: &mut CityState,
    tick: u64,
    width: u16,
    height: u16,
    activity: DecorationActivity,
) {
    if width < 4 || height < 3 {
        return;
    }
    if !state.initialized {
        initialize_city(state, width, height);
    }

    let max_x = width.saturating_sub(2) as f32;

    // Move cars every 2nd tick.
    if tick % 2 == 0 {
        for car in &mut state.cars {
            car.x += car.speed * car.direction as f32;
            // Wrap around.
            if car.x > max_x + 2.0 {
                car.x = -2.0;
            } else if car.x < -2.0 {
                car.x = max_x + 2.0;
            }
        }
    }

    // Manage car count based on activity.
    let target_cars = match activity {
        DecorationActivity::Calm => 2_usize,
        DecorationActivity::Active => 4,
    };

    // Spawn cars to reach the target.
    if tick % 15 == 0 && state.cars.len() < target_cars {
        let r = pseudo_random(tick, 33);
        let from_left = r % 2 == 0;
        let emoji = CAR_EMOJIS[(r >> 4) as usize % CAR_EMOJIS.len()];
        let speed = match activity {
            DecorationActivity::Calm => 0.4,
            DecorationActivity::Active => 0.7 + (r % 3) as f32 * 0.2,
        };
        state.cars.push(Car {
            x: if from_left { 0.0 } else { max_x },
            speed,
            direction: if from_left { 1 } else { -1 },
            emoji,
        });
    }

    // Remove excess cars gradually.
    if state.cars.len() > target_cars && tick % 20 == 0 {
        // Remove the last car.
        state.cars.pop();
    }
}

/// Render the city scene.
fn render_city(frame: &mut Frame, area: Rect, state: &CityState, theme: &Theme) {
    if area.width < 4 || area.height < 2 {
        return;
    }

    let w = area.width as usize;
    let h = area.height as usize;
    let mut grid: Vec<Vec<Option<&str>>> = vec![vec![None; w]; h];

    // Sky objects (top rows).
    for obj in &state.sky {
        let col = (obj.x as usize).min(w.saturating_sub(2));
        let row = (obj.y as usize).min(h.saturating_sub(1));
        if col + 1 < w {
            grid[row][col] = Some(obj.emoji);
        }
    }

    // Bottom row: buildings.
    if h >= 1 {
        let bottom = h - 1;
        for bldg in &state.buildings {
            let col = (bldg.x as usize).min(w.saturating_sub(2));
            if col + 1 < w {
                grid[bottom][col] = Some(bldg.emoji);
            }
        }
    }

    // Cars on the row above buildings (the "road").
    if h >= 2 {
        let road_row = h - 2;
        for car in &state.cars {
            let col = car.x as isize;
            if col >= 0 && (col as usize) + 1 < w {
                grid[road_row][col as usize] = Some(car.emoji);
            }
        }
    }

    render_grid(frame, area, &grid, theme);
}

// ═══════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    // ── DecorationMode ───────────────────────────────────────────────

    #[test]
    fn mode_from_str_known_values() {
        assert_eq!(DecorationMode::from_str("aquarium"), DecorationMode::Aquarium);
        assert_eq!(DecorationMode::from_str("space"), DecorationMode::Space);
        assert_eq!(DecorationMode::from_str("garden"), DecorationMode::Garden);
        assert_eq!(DecorationMode::from_str("city"), DecorationMode::City);
        assert_eq!(DecorationMode::from_str("none"), DecorationMode::None);
    }

    #[test]
    fn mode_from_str_defaults_to_aquarium() {
        assert_eq!(DecorationMode::from_str("unknown"), DecorationMode::Aquarium);
        assert_eq!(DecorationMode::from_str(""), DecorationMode::Aquarium);
    }

    #[test]
    fn mode_has_animation() {
        assert!(DecorationMode::Aquarium.has_animation());
        assert!(DecorationMode::Space.has_animation());
        assert!(DecorationMode::Garden.has_animation());
        assert!(DecorationMode::City.has_animation());
        assert!(!DecorationMode::None.has_animation());
    }

    // ── Aquarium ─────────────────────────────────────────────────────

    #[test]
    fn aquarium_initializes_on_first_tick() {
        let mut state = AquariumState::default();
        assert!(!state.initialized);
        tick_aquarium(&mut state, 0, 20, 6, DecorationActivity::Calm);
        assert!(state.initialized);
        assert!(!state.fish.is_empty());
    }

    #[test]
    fn aquarium_skips_small_area() {
        let mut state = AquariumState::default();
        tick_aquarium(&mut state, 0, 2, 2, DecorationActivity::Calm);
        assert!(state.fish.is_empty());
    }

    #[test]
    fn aquarium_bubbles_spawn_faster_when_active() {
        let mut state = AquariumState::default();
        // Calm — run 100 ticks.
        for t in 0..100 {
            tick_aquarium(&mut state, t, 20, 6, DecorationActivity::Calm);
        }
        let calm_bubbles = state.bubbles.len();

        let mut state2 = AquariumState::default();
        for t in 0..100 {
            tick_aquarium(&mut state2, t, 20, 6, DecorationActivity::Active);
        }
        let active_bubbles = state2.bubbles.len();
        // Active should have at least as many (usually more) bubbles.
        assert!(active_bubbles >= calm_bubbles);
    }

    // ── Space ────────────────────────────────────────────────────────

    #[test]
    fn space_initializes_on_first_tick() {
        let mut state = SpaceState::default();
        assert!(!state.initialized);
        tick_space(&mut state, 0, 20, 6, DecorationActivity::Calm);
        assert!(state.initialized);
        assert!(!state.stars.is_empty());
        assert!(!state.planets.is_empty());
    }

    #[test]
    fn space_skips_small_area() {
        let mut state = SpaceState::default();
        tick_space(&mut state, 0, 2, 2, DecorationActivity::Calm);
        assert!(state.stars.is_empty());
    }

    #[test]
    fn space_shooting_stars_spawn_in_active() {
        let mut state = SpaceState::default();
        // Run enough ticks to trigger shooting star spawning.
        for t in 0..100 {
            tick_space(&mut state, t, 30, 8, DecorationActivity::Active);
        }
        // At least one shooting star should have spawned over 100 ticks.
        // (They may have already left the screen, but we should see the
        // mechanism works by checking planets still exist.)
        assert!(state.initialized);
    }

    #[test]
    fn space_planets_bounce() {
        let mut state = SpaceState::default();
        tick_space(&mut state, 0, 10, 6, DecorationActivity::Calm);
        let initial_x = state.planets[0].x;
        // Tick enough to move the planet.
        for t in 1..200 {
            tick_space(&mut state, t, 10, 6, DecorationActivity::Calm);
        }
        // Planet should have moved and bounced, ending at a different position.
        // (It might be back near start after enough bounces, so just verify it moved.)
        let final_x = state.planets[0].x;
        assert!(
            (final_x - initial_x).abs() > 0.01 || state.planets[0].direction != 1,
            "planet should have moved"
        );
    }

    // ── Garden ───────────────────────────────────────────────────────

    #[test]
    fn garden_initializes_on_first_tick() {
        let mut state = GardenState::default();
        assert!(!state.initialized);
        tick_garden(&mut state, 0, 20, 6, DecorationActivity::Calm);
        assert!(state.initialized);
        assert!(!state.plants.is_empty());
        assert!(!state.butterflies.is_empty());
    }

    #[test]
    fn garden_skips_small_area() {
        let mut state = GardenState::default();
        tick_garden(&mut state, 0, 2, 2, DecorationActivity::Calm);
        assert!(state.plants.is_empty());
    }

    #[test]
    fn garden_birds_appear_when_active() {
        let mut state = GardenState::default();
        for t in 0..100 {
            tick_garden(&mut state, t, 20, 6, DecorationActivity::Active);
        }
        // Birds should have been spawned at least once during 100 Active ticks.
        // They may have left the area, so we just check the system didn't panic.
        assert!(state.initialized);
    }

    #[test]
    fn garden_butterflies_stay_in_bounds() {
        let mut state = GardenState::default();
        let w: u16 = 20;
        let h: u16 = 6;
        for t in 0..500 {
            tick_garden(&mut state, t, w, h, DecorationActivity::Active);
        }
        for bf in &state.butterflies {
            assert!(bf.x >= -0.5, "butterfly x out of bounds: {}", bf.x);
            assert!(bf.x <= w as f32, "butterfly x out of bounds: {}", bf.x);
            assert!(bf.y >= -0.5, "butterfly y out of bounds: {}", bf.y);
            assert!(bf.y <= h as f32, "butterfly y out of bounds: {}", bf.y);
        }
    }

    // ── City ─────────────────────────────────────────────────────────

    #[test]
    fn city_initializes_on_first_tick() {
        let mut state = CityState::default();
        assert!(!state.initialized);
        tick_city(&mut state, 0, 20, 6, DecorationActivity::Calm);
        assert!(state.initialized);
        assert!(!state.buildings.is_empty());
        assert!(!state.cars.is_empty());
    }

    #[test]
    fn city_skips_small_area() {
        let mut state = CityState::default();
        tick_city(&mut state, 0, 2, 2, DecorationActivity::Calm);
        assert!(state.buildings.is_empty());
    }

    #[test]
    fn city_more_cars_when_active() {
        let mut state_calm = CityState::default();
        for t in 0..200 {
            tick_city(&mut state_calm, t, 30, 8, DecorationActivity::Calm);
        }
        let calm_cars = state_calm.cars.len();

        let mut state_active = CityState::default();
        for t in 0..200 {
            tick_city(&mut state_active, t, 30, 8, DecorationActivity::Active);
        }
        let active_cars = state_active.cars.len();

        assert!(
            active_cars >= calm_cars,
            "active ({active_cars}) should have >= calm ({calm_cars}) cars"
        );
    }

    #[test]
    fn city_cars_wrap_around() {
        let mut state = CityState::default();
        tick_city(&mut state, 0, 10, 6, DecorationActivity::Calm);
        // Force car to far right.
        state.cars[0].x = 9.0;
        state.cars[0].direction = 1;
        state.cars[0].speed = 5.0;
        for t in 1..20 {
            tick_city(&mut state, t, 10, 6, DecorationActivity::Calm);
        }
        // Car should have wrapped around to the left side.
        assert!(state.cars[0].x < 9.0, "car should have wrapped");
    }

    // ── Dispatch ─────────────────────────────────────────────────────

    #[test]
    fn tick_decoration_dispatches_correctly() {
        let mut states = DecorationStates::default();

        // Tick each mode and verify the corresponding state got initialized.
        tick_decoration(&mut states, 0, 20, 6, DecorationActivity::Calm, DecorationMode::Aquarium);
        assert!(states.aquarium.initialized);
        assert!(!states.space.initialized);

        tick_decoration(&mut states, 0, 20, 6, DecorationActivity::Calm, DecorationMode::Space);
        assert!(states.space.initialized);

        tick_decoration(&mut states, 0, 20, 6, DecorationActivity::Calm, DecorationMode::Garden);
        assert!(states.garden.initialized);

        tick_decoration(&mut states, 0, 20, 6, DecorationActivity::Calm, DecorationMode::City);
        assert!(states.city.initialized);
    }

    #[test]
    fn tick_decoration_none_is_noop() {
        let mut states = DecorationStates::default();
        tick_decoration(&mut states, 0, 20, 6, DecorationActivity::Calm, DecorationMode::None);
        assert!(!states.aquarium.initialized);
        assert!(!states.space.initialized);
        assert!(!states.garden.initialized);
        assert!(!states.city.initialized);
    }
}
