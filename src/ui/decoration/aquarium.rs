//! Aquarium decoration mode 🐠🐟🐡🐙🦀🦑 — fish swimming across the panel
//! with rising bubbles and a coral-lined floor.

use ratatui::Frame;
use ratatui::layout::Rect;

use crate::theme::Theme;

use super::{DecorationActivity, render_grid};

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
pub(super) fn tick_aquarium(
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
    if ui_tick.is_multiple_of(3) {
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
            if ui_tick.is_multiple_of(15) && usable_height > 2 {
                let raw = fish.y as i16 + fish.direction as i16;
                let new_y = (raw.max(0) as u16).min(usable_height.saturating_sub(2));
                fish.y = new_y;
            }
        }
    }

    // Float bubbles upward every 2nd tick.
    if ui_tick.is_multiple_of(2) {
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
    if ui_tick.is_multiple_of(spawn_chance) && state.bubbles.len() < 8 {
        let x = ((ui_tick * 7 + 3) % width as u64) as u16;
        let y = height.saturating_sub(2) as f32;
        state.bubbles.push(Bubble { x, y });
    }
}

/// Render the aquarium into the given area.
pub(super) fn render_aquarium(frame: &mut Frame, area: Rect, state: &AquariumState, theme: &Theme) {
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
