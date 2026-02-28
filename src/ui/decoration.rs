//! Decoration rendering for the worktree panel's empty space.
//!
//! Currently supports an "aquarium" mode with animated fish and bubbles.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::theme::Theme;

/// Decoration mode parsed from config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecorationMode {
    Aquarium,
    None,
}

impl DecorationMode {
    pub fn from_str(s: &str) -> Self {
        match s {
            "none" => Self::None,
            _ => Self::Aquarium,
        }
    }
}

/// Activity level affecting bubble frequency and fish speed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AquariumActivity {
    Calm,
    Active,
}

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

const FISH_EMOJIS: &[&str] = &["\u{1F420}", "\u{1F41F}", "\u{1F421}", "\u{1F419}", "\u{1F980}", "\u{1F991}"];
// 🐠 🐟 🐡 🐙 🦀 🦑

const CORAL: &str = "\u{1FAB8}"; // 🪸
const BUBBLE: &str = "\u{1FAE7}"; // 🫧

/// Initialize the aquarium with fish placed evenly across the area.
fn initialize(state: &mut AquariumState, width: u16, height: u16) {
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
        state.fish.push(Fish { x, y, emoji, direction, speed });
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
    activity: AquariumActivity,
) {
    if width < 4 || height < 3 {
        return;
    }

    if !state.initialized {
        initialize(state, width, height);
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
                let new_y = (raw.max(0) as u16)
                    .min(usable_height.saturating_sub(2));
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
        AquariumActivity::Calm => 12,
        AquariumActivity::Active => 5,
    };
    if ui_tick % spawn_chance == 0 && state.bubbles.len() < 8 {
        let x = ((ui_tick * 7 + 3) % width as u64) as u16;
        let y = height.saturating_sub(2) as f32;
        state.bubbles.push(Bubble { x, y });
    }
}

/// Render the aquarium into the given area.
pub fn render_aquarium(frame: &mut Frame, area: Rect, state: &AquariumState, theme: &Theme) {
    if area.width < 4 || area.height < 2 {
        return;
    }

    let inner_width = area.width;
    let inner_height = area.height;

    // Build a grid of cells; each cell is either empty or an emoji.
    // Emoji take 2 columns in the terminal.
    let mut grid: Vec<Vec<Option<&str>>> = vec![vec![None; inner_width as usize]; inner_height as usize];

    // Place coral on the bottom row.
    if inner_height >= 1 {
        let bottom = (inner_height - 1) as usize;
        let mut col = 0;
        while col + 1 < inner_width as usize {
            grid[bottom][col] = Some(CORAL);
            col += 3; // space corals out
        }
    }

    // Place fish.
    for fish in &state.fish {
        let col = (fish.x as u16).min(inner_width.saturating_sub(2)) as usize;
        let row = (fish.y as usize).min(inner_height.saturating_sub(2) as usize);
        if row < grid.len() && col + 1 < inner_width as usize {
            grid[row][col] = Some(fish.emoji);
        }
    }

    // Place bubbles.
    for bubble in &state.bubbles {
        let col = (bubble.x as usize).min(inner_width.saturating_sub(2) as usize);
        let row = (bubble.y as usize).min(inner_height.saturating_sub(1) as usize);
        if row < grid.len() && col + 1 < inner_width as usize {
            grid[row][col] = Some(BUBBLE);
        }
    }

    // Render each row as a Line of Spans.
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

/// Dispatch decoration rendering based on config mode.
pub fn render_decoration(
    frame: &mut Frame,
    area: Rect,
    state: &AquariumState,
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
            render_aquarium(frame, inner, state, theme);
        }
        DecorationMode::None => {}
    }
}
