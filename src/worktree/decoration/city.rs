//! シティ装飾モード 🏢🚗🌙 — ビルと信号機が並ぶスカイラインに、道を走る車と
//! 空に浮かぶ月・星を添える。

use ratatui::Frame;
use ratatui::layout::Rect;

use crate::theme::Theme;

use super::{DecorationActivity, pseudo_random, render_grid};

/// シティのスカイラインを構成するビル。
#[derive(Debug, Clone)]
pub struct Building {
    pub x: u16,
    pub emoji: &'static str,
}

/// 道路を走る車。
#[derive(Debug, Clone)]
pub struct Car {
    pub x: f32,
    pub speed: f32,
    pub direction: i8,
    pub emoji: &'static str,
}

/// 空の装飾（月、星）。
#[derive(Debug, Clone)]
pub struct SkyObject {
    pub x: u16,
    pub y: u16,
    pub emoji: &'static str,
}

/// シティアニメーションの状態全体。
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

    // 最下段に並ぶビル。
    state.buildings.clear();
    let mut col: u16 = 0;
    let mut idx = 0;
    while col + 1 < width {
        // 4つに1つのスロットは信号機、それ以外はビルにする。
        let emoji = if idx % 5 == 3 {
            TRAFFIC_LIGHT
        } else {
            BUILDING_EMOJIS[idx % BUILDING_EMOJIS.len()]
        };
        state.buildings.push(Building { x: col, emoji });
        col += 3;
        idx += 1;
    }

    // 空のオブジェクト: 月と星をいくつか。
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

    // 初期配置の車。
    state.cars.clear();
    state.cars.push(Car {
        x: 2.0,
        speed: 0.5,
        direction: 1,
        emoji: CAR_EMOJIS[0],
    });

    state.initialized = true;
}

/// シティアニメーションを1ティック進める。
pub(super) fn tick_city(
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

    // 2ティックごとに車を移動させる。
    if tick.is_multiple_of(2) {
        for car in &mut state.cars {
            car.x += car.speed * car.direction as f32;
            // 反対側へ回り込ませる。
            if car.x > max_x + 2.0 {
                car.x = -2.0;
            } else if car.x < -2.0 {
                car.x = max_x + 2.0;
            }
        }
    }

    // activity に応じて車の台数を調整する。
    let target_cars = match activity {
        DecorationActivity::Calm => 2_usize,
        DecorationActivity::Active => 4,
    };

    // 目標台数に達するまで車を生成する。
    if tick.is_multiple_of(15) && state.cars.len() < target_cars {
        let r = pseudo_random(tick, 33);
        let from_left = r.is_multiple_of(2);
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

    // 超過分の車を少しずつ減らす。
    if state.cars.len() > target_cars && tick.is_multiple_of(20) {
        // 末尾の車を取り除く。
        state.cars.pop();
    }
}

pub(super) fn render_city(frame: &mut Frame, area: Rect, state: &CityState, theme: &Theme) {
    if area.width < 4 || area.height < 2 {
        return;
    }

    let w = area.width as usize;
    let h = area.height as usize;
    let mut grid: Vec<Vec<Option<&str>>> = vec![vec![None; w]; h];

    // 空のオブジェクト（上段）。
    for obj in &state.sky {
        let col = (obj.x as usize).min(w.saturating_sub(2));
        let row = (obj.y as usize).min(h.saturating_sub(1));
        if col + 1 < w {
            grid[row][col] = Some(obj.emoji);
        }
    }

    // 最下段: ビル。
    if h >= 1 {
        let bottom = h - 1;
        for bldg in &state.buildings {
            let col = (bldg.x as usize).min(w.saturating_sub(2));
            if col + 1 < w {
                grid[bottom][col] = Some(bldg.emoji);
            }
        }
    }

    // ビルの1つ上の段に車を置く（道路）。
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
