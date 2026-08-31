//! 装飾モードのディスパッチと、各アニメーションモードの tick の挙動
//! （初期化、activity に応じた生成、境界）のテスト。

use super::*;

#[test]
fn mode_from_str_known_values() {
    assert_eq!(
        DecorationMode::from_str("aquarium"),
        DecorationMode::Aquarium
    );
    assert_eq!(DecorationMode::from_str("space"), DecorationMode::Space);
    assert_eq!(DecorationMode::from_str("garden"), DecorationMode::Garden);
    assert_eq!(DecorationMode::from_str("city"), DecorationMode::City);
    assert_eq!(DecorationMode::from_str("none"), DecorationMode::None);
}

#[test]
fn mode_from_str_defaults_to_aquarium() {
    assert_eq!(
        DecorationMode::from_str("unknown"),
        DecorationMode::Aquarium
    );
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
    // Calm — 100ティック実行する。
    for t in 0..100 {
        tick_aquarium(&mut state, t, 20, 6, DecorationActivity::Calm);
    }
    let calm_bubbles = state.bubbles.len();

    let mut state2 = AquariumState::default();
    for t in 0..100 {
        tick_aquarium(&mut state2, t, 20, 6, DecorationActivity::Active);
    }
    let active_bubbles = state2.bubbles.len();
    assert!(
        active_bubbles > calm_bubbles,
        "active ({active_bubbles}) は calm ({calm_bubbles}) より多く泡を出すはず"
    );
}

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
fn space_planets_turn_around_at_the_edge() {
    let mut state = SpaceState::default();
    tick_space(&mut state, 0, 10, 6, DecorationActivity::Calm);
    // 端に押し付けてから進ませる。反転を見るのが目的なので、偶然の往復では代用しない。
    state.planets[0].x = 9.9;
    state.planets[0].direction = 1;
    for t in 1..200 {
        tick_space(&mut state, t, 10, 6, DecorationActivity::Calm);
        if state.planets[0].direction == -1 {
            return;
        }
    }
    panic!("端に着いても向きが変わらなかった");
}

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
        active_cars > calm_cars,
        "active ({active_cars}) は calm ({calm_cars}) より多く車を出すはず"
    );
}

#[test]
fn city_cars_wrap_around() {
    let mut state = CityState::default();
    tick_city(&mut state, 0, 10, 6, DecorationActivity::Calm);
    // 車を右端まで強制的に動かす。
    state.cars[0].x = 9.0;
    state.cars[0].direction = 1;
    state.cars[0].speed = 5.0;
    for t in 1..20 {
        tick_city(&mut state, t, 10, 6, DecorationActivity::Calm);
    }
    // 車は左側へ回り込んでいるはず。
    assert!(state.cars[0].x < 9.0, "car should have wrapped");
}

#[test]
fn tick_decoration_routes_each_mode_to_its_own_state() {
    let mut states = DecorationStates::default();

    // 各モードをティックし、対応する状態が初期化されたことを確認する。
    tick_decoration(
        &mut states,
        0,
        20,
        6,
        DecorationActivity::Calm,
        DecorationMode::Aquarium,
    );
    assert!(states.aquarium.initialized);
    assert!(!states.space.initialized);

    tick_decoration(
        &mut states,
        0,
        20,
        6,
        DecorationActivity::Calm,
        DecorationMode::Space,
    );
    assert!(states.space.initialized);

    tick_decoration(
        &mut states,
        0,
        20,
        6,
        DecorationActivity::Calm,
        DecorationMode::Garden,
    );
    assert!(states.garden.initialized);

    tick_decoration(
        &mut states,
        0,
        20,
        6,
        DecorationActivity::Calm,
        DecorationMode::City,
    );
    assert!(states.city.initialized);
}

#[test]
fn tick_decoration_none_is_noop() {
    let mut states = DecorationStates::default();
    tick_decoration(
        &mut states,
        0,
        20,
        6,
        DecorationActivity::Calm,
        DecorationMode::None,
    );
    assert!(!states.aquarium.initialized);
    assert!(!states.space.initialized);
    assert!(!states.garden.initialized);
    assert!(!states.city.initialized);
}
