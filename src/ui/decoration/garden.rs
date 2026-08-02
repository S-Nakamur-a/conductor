//! ガーデン装飾モード 🌳🌸🦋🐦 — 植物を並べた縁取りに、漂う蝶や蜂、
//! 時折通り過ぎる鳥を添える。

use ratatui::Frame;
use ratatui::layout::Rect;

use crate::theme::Theme;

use super::{DecorationActivity, pseudo_random, render_grid};

/// ガーデンの地面に固定して生える植物。
#[derive(Debug, Clone)]
pub struct GardenPlant {
    pub x: u16,
    pub emoji: &'static str,
}

/// 空中を漂う蝶。
#[derive(Debug, Clone)]
pub struct Butterfly {
    pub x: f32,
    pub y: f32,
    pub dx: f32,
    pub dy: f32,
}

/// 水平に飛ぶ鳥。
#[derive(Debug, Clone)]
pub struct Bird {
    pub x: f32,
    pub y: u16,
    pub speed: f32,
    pub direction: i8,
}

/// ガーデンアニメーションの状態全体。
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

    // 最下段に植物を並べる: 木、花、ハーブ。
    state.plants.clear();
    let all_plants: Vec<&str> = TREE_EMOJIS
        .iter()
        .chain(FLOWER_EMOJIS.iter())
        .copied()
        .collect();
    let mut col: u16 = 0;
    let mut idx = 0;
    while col + 1 < width {
        // 名前付きの植物とハーブの詰め物を交互に配置する。
        let emoji = if idx % 3 == 0 {
            HERB
        } else {
            all_plants[idx % all_plants.len()]
        };
        state.plants.push(GardenPlant { x: col, emoji });
        col += 3;
        idx += 1;
    }

    // 初期配置の蝶。
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

/// ガーデンアニメーションを1ティック進める。
pub(super) fn tick_garden(
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
    // 最下段は植物用に空けておく。使用可能な行は 0 .. height-2。
    let max_y = height.saturating_sub(2) as f32;

    // 2ティックごとに蝶を移動させる。
    if tick.is_multiple_of(2) {
        for (i, bf) in state.butterflies.iter_mut().enumerate() {
            bf.x += bf.dx;
            bf.y += bf.dy;

            // 境界で跳ね返す。
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

            // 自然な動きに見せるため、時々向きを変える。
            if tick % 11 == (i as u64 % 11) {
                let r = pseudo_random(tick, i as u64 + 100);
                bf.dx = ((r % 7) as f32 - 3.0) * 0.15;
                bf.dy = (((r >> 8) % 5) as f32 - 2.0) * 0.15;
            }
        }
    }

    // 鳥 — 水平に飛び、エリア外へ抜けていく。
    if tick.is_multiple_of(2) {
        for bird in &mut state.birds {
            bird.x += bird.speed * bird.direction as f32;
        }
        state
            .birds
            .retain(|b| b.x >= -2.0 && (b.x as u16) < width + 2);
    }

    // activity に応じて蝶や蜂を生成する。
    let target_count = match activity {
        DecorationActivity::Calm => 2_usize,
        DecorationActivity::Active => 4,
    };
    if tick.is_multiple_of(20) && state.butterflies.len() < target_count {
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

    // アクティブモードでは、時々鳥を生成する。
    if activity == DecorationActivity::Active && tick.is_multiple_of(30) && state.birds.len() < 2 {
        let r = pseudo_random(tick, 77);
        let y = ((r % height.saturating_sub(2) as u64) as u16).min(height / 2);
        let from_left = r.is_multiple_of(2);
        state.birds.push(Bird {
            x: if from_left { 0.0 } else { max_x },
            y,
            speed: 0.8,
            direction: if from_left { 1 } else { -1 },
        });
    }
}

/// ガーデンのシーンを描画する。
pub(super) fn render_garden(frame: &mut Frame, area: Rect, state: &GardenState, theme: &Theme) {
    if area.width < 4 || area.height < 2 {
        return;
    }

    let w = area.width as usize;
    let h = area.height as usize;
    let mut grid: Vec<Vec<Option<&str>>> = vec![vec![None; w]; h];

    // 最下段: 植物。
    if h >= 1 {
        let bottom = h - 1;
        for plant in &state.plants {
            let col = (plant.x as usize).min(w.saturating_sub(2));
            if col + 1 < w {
                grid[bottom][col] = Some(plant.emoji);
            }
        }
    }

    // 蝶・蜂。
    for (i, bf) in state.butterflies.iter().enumerate() {
        let col = (bf.x as usize).min(w.saturating_sub(2));
        let row = (bf.y as usize).min(h.saturating_sub(2));
        if col + 1 < w && row < h {
            // 変化をつけるため3匹に1匹は蜂にする。
            let emoji = if i % 3 == 2 {
                BEE_EMOJI
            } else {
                BUTTERFLY_EMOJI
            };
            grid[row][col] = Some(emoji);
        }
    }

    // 鳥。
    for bird in &state.birds {
        let col = (bird.x as usize).min(w.saturating_sub(2));
        let row = (bird.y as usize).min(h.saturating_sub(2));
        if col + 1 < w && col < w && row < h {
            grid[row][col] = Some(BIRD_EMOJI);
        }
    }

    render_grid(frame, area, &grid, theme);
}
