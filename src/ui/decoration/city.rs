//! City decoration mode 🏢🚗🌙 — a building/traffic-light skyline with cars
//! driving along the road and a moon/stars in the sky.

use ratatui::Frame;
use ratatui::layout::Rect;

use crate::theme::Theme;

use super::{DecorationActivity, pseudo_random, render_grid};

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

    // Move cars every 2nd tick.
    if tick.is_multiple_of(2) {
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

    // Remove excess cars gradually.
    if state.cars.len() > target_cars && tick.is_multiple_of(20) {
        // Remove the last car.
        state.cars.pop();
    }
}

/// Render the city scene.
pub(super) fn render_city(frame: &mut Frame, area: Rect, state: &CityState, theme: &Theme) {
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
