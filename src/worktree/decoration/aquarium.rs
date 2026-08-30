//! アクアリウム装飾モード 🐠🐟🐡🐙🦀🦑 — パネル内を泳ぐ魚と、立ち上る泡、
//! サンゴが並ぶ底面。

use ratatui::Frame;
use ratatui::layout::Rect;

use crate::theme::Theme;

use super::{DecorationActivity, render_grid};

/// アクアリウムを泳ぐ魚。
#[derive(Debug, Clone)]
pub struct Fish {
    pub x: f32,
    pub y: u16,
    pub emoji: &'static str,
    pub direction: i8,
    pub speed: f32,
}

/// 上へ昇っていく泡。
#[derive(Debug, Clone)]
pub struct Bubble {
    pub x: u16,
    pub y: f32,
}

/// アクアリウムアニメーションの状態全体。
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

/// エリア内に魚を均等に配置してアクアリウムを初期化する。
fn initialize_aquarium(state: &mut AquariumState, width: u16, height: u16) {
    if width < 4 || height < 3 {
        state.initialized = true;
        return;
    }

    let fish_count = 5.min((width / 4) as usize).max(2);
    state.fish.clear();
    // 行0は上部の余白、最終行はサンゴのために空けておく
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

/// アクアリウムアニメーションを1ティック進める。
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

    // ゆったりしたペースにするため、3ティックごとに魚を移動させる。
    if ui_tick.is_multiple_of(3) {
        let max_x = width.saturating_sub(2) as f32;
        let usable_height = height.saturating_sub(1);
        for fish in &mut state.fish {
            fish.x += fish.speed * fish.direction as f32;
            // 壁で跳ね返す。
            if fish.x < 0.0 {
                fish.x = 0.0;
                fish.direction = 1;
            } else if fish.x > max_x {
                fish.x = max_x;
                fish.direction = -1;
            }
            // 時々、上下の位置を変える。
            if ui_tick.is_multiple_of(15) && usable_height > 2 {
                let raw = fish.y as i16 + fish.direction as i16;
                let new_y = (raw.max(0) as u16).min(usable_height.saturating_sub(2));
                fish.y = new_y;
            }
        }
    }

    // 2ティックごとに泡を上へ浮かせる。
    if ui_tick.is_multiple_of(2) {
        for bubble in &mut state.bubbles {
            bubble.y -= 0.3;
        }
        // 画面外まで浮いた泡を取り除く。
        state.bubbles.retain(|b| b.y > 0.0);
    }

    // activity レベルに応じて新しい泡を生成する。
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

/// 指定エリアにアクアリウムを描画する。
pub(super) fn render_aquarium(frame: &mut Frame, area: Rect, state: &AquariumState, theme: &Theme) {
    if area.width < 4 || area.height < 2 {
        return;
    }

    let w = area.width as usize;
    let h = area.height as usize;

    let mut grid: Vec<Vec<Option<&str>>> = vec![vec![None; w]; h];

    // 最下段にサンゴを配置する。
    if h >= 1 {
        let bottom = h - 1;
        let mut col = 0;
        while col + 1 < w {
            grid[bottom][col] = Some(CORAL);
            col += 3;
        }
    }

    // 魚を配置する。
    for fish in &state.fish {
        let col = (fish.x as usize).min(w.saturating_sub(2));
        let row = (fish.y as usize).min(h.saturating_sub(2));
        if row < h && col + 1 < w {
            grid[row][col] = Some(fish.emoji);
        }
    }

    // 泡を配置する。
    for bubble in &state.bubbles {
        let col = (bubble.x as usize).min(w.saturating_sub(2));
        let row = (bubble.y as usize).min(h.saturating_sub(1));
        if row < h && col + 1 < w {
            grid[row][col] = Some(BUBBLE_EMOJI);
        }
    }

    render_grid(frame, area, &grid, theme);
}
