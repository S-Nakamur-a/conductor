//! Tests for `LayoutCache::update` and `accordion_widths`.

use ratatui::layout::Rect;

use super::{LayoutCache, accordion_widths};
use crate::app::Focus;
use crate::config::LayoutConfig;

/// Build a minimal LayoutConfig with the given proportions.
fn layout(explorer: u16, viewer: u16, terminal: u16) -> LayoutConfig {
    LayoutConfig {
        explorer_width_pct: explorer,
        viewer_width_pct: viewer,
        terminal_split_pct: terminal,
        explorer_split_pct: 50,
    }
}

/// A non-zero Rect large enough to produce non-trivial layout splits.
fn rect(w: u16, h: u16) -> Rect {
    Rect::new(0, 0, w, h)
}

// ── LayoutCache::update ──────────────────────────────────────────

#[test]
fn layout_cache_update_returns_true_first_call() {
    let mut cache = LayoutCache::default();
    let changed = cache.update(rect(200, 50), None, false, &layout(24, 38, 80), 80);
    assert!(changed, "first update must recompute");
}

#[test]
fn layout_cache_update_returns_false_on_identical_second_call() {
    let mut cache = LayoutCache::default();
    let cfg = layout(24, 38, 80);
    cache.update(rect(200, 50), None, false, &cfg, 80);
    let changed = cache.update(rect(200, 50), None, false, &cfg, 80);
    assert!(!changed, "identical inputs must not recompute");
}

#[test]
fn layout_cache_invalidates_on_frame_area_change() {
    let mut cache = LayoutCache::default();
    let cfg = layout(24, 38, 80);
    cache.update(rect(200, 50), None, false, &cfg, 80);
    let changed = cache.update(rect(201, 50), None, false, &cfg, 80);
    assert!(changed, "frame_area change must invalidate");
}

#[test]
fn layout_cache_invalidates_on_explorer_pct_change() {
    let mut cache = LayoutCache::default();
    cache.update(rect(200, 50), None, false, &layout(24, 38, 80), 80);
    let changed = cache.update(rect(200, 50), None, false, &layout(30, 38, 80), 80);
    assert!(changed, "explorer_width_pct change must invalidate");
}

#[test]
fn layout_cache_invalidates_on_viewer_pct_change() {
    let mut cache = LayoutCache::default();
    cache.update(rect(200, 50), None, false, &layout(24, 38, 80), 80);
    let changed = cache.update(rect(200, 50), None, false, &layout(24, 42, 80), 80);
    assert!(changed, "viewer_width_pct change must invalidate");
}

#[test]
fn layout_cache_invalidates_on_terminal_split_change() {
    // The terminal split now arrives as a runtime parameter (grow/shrink
    // shell), so vary that argument rather than the config field.
    let mut cache = LayoutCache::default();
    let cfg = layout(24, 38, 80);
    cache.update(rect(200, 50), None, false, &cfg, 80);
    let changed = cache.update(rect(200, 50), None, false, &cfg, 70);
    assert!(changed, "terminal_split_pct change must invalidate");
}

#[test]
fn layout_cache_invalidates_on_explorer_split_change() {
    let mut cache = LayoutCache::default();
    let mut cfg = layout(24, 38, 80);
    cfg.explorer_split_pct = 50;
    cache.update(rect(200, 50), None, false, &cfg, 80);
    cfg.explorer_split_pct = 30;
    let changed = cache.update(rect(200, 50), None, false, &cfg, 80);
    assert!(changed, "explorer_split_pct change must invalidate");
    // The mid-point moves up when the file tree shrinks.
    assert!(cache.explorer_mid_y > 0);
}

// ── accordion_widths: abnormal percentages ───────────────────────

#[test]
fn accordion_widths_does_not_panic_on_large_percentages() {
    // Percentages exceeding 100 must not panic (Percentage constraint clamps).
    let _ = accordion_widths(None, 200, 240, 0);
    let _ = accordion_widths(None, 100, 60, 60);
}

#[test]
fn terminal_split_pct_over_100_does_not_panic() {
    // terminal_split_pct > 100 makes shell_pct saturate to 0; must not panic.
    let mut cache = LayoutCache::default();
    let _ = cache.update(rect(200, 50), None, false, &layout(24, 38, 200), 200);
}

#[test]
fn maximized_editor_takes_full_width_via_explorer_slot() {
    // render_ui unions the explorer+viewer columns into the editor area, so
    // the maximized editor puts all width on the explorer slot (viewer 0),
    // collapsing the terminal column to zero remaining.
    let (left, explorer, viewer) = accordion_widths(Some(Focus::Editor), 120, 24, 38);
    assert_eq!((left, explorer, viewer), (0, 120, 0));
}

#[test]
fn default_layout_uses_configured_percentages() {
    // With default percentages (24/38) and 200-column terminal.
    let (left, explorer, viewer) = accordion_widths(None, 200, 24, 38);
    assert_eq!(left, 0, "worktree column is always hidden");
    assert_eq!(explorer, 48, "200 * 24 / 100 = 48");
    assert_eq!(viewer, 76, "200 * 38 / 100 = 76");
}

#[test]
fn custom_percentages_are_respected() {
    // Wider explorer (30%) and narrower viewer (30%).
    let (left, explorer, viewer) = accordion_widths(None, 100, 30, 30);
    assert_eq!(left, 0);
    assert_eq!(explorer, 30);
    assert_eq!(viewer, 30);
}

// ── Menu bar row ─────────────────────────────────────────────────

#[test]
fn menu_bar_sits_directly_under_the_title_bar() {
    let mut cache = LayoutCache::default();
    cache.update(rect(200, 50), None, false, &layout(24, 38, 80), 80);
    assert_eq!(cache.title_area.y, 0);
    assert_eq!(cache.title_area.height, 1);
    assert_eq!(
        cache.menubar_area.y,
        cache.title_area.y + cache.title_area.height,
        "the menu bar must be the row immediately below the title bar"
    );
    assert_eq!(cache.menubar_area.height, 1);
    assert_eq!(
        cache.menubar_area.width, 200,
        "the bar spans the full width so its blank stretch is still clickable"
    );
}

#[test]
fn menu_bar_stays_visible_while_a_panel_is_maximized() {
    // Unlike the worktree strip, which collapses to give the expanded panel
    // its rows back. A menu reachable only after un-maximizing is a menu that
    // stops being used.
    let mut cache = LayoutCache::default();
    cache.update(
        rect(200, 50),
        Some(Focus::Explorer),
        false,
        &layout(24, 38, 80),
        80,
    );
    assert_eq!(cache.menubar_area.height, 1);
    assert_eq!(cache.wtbar_area.height, 0, "worktree strip still collapses");
}

#[test]
fn menu_bar_row_comes_out_of_the_main_area() {
    // The bar takes its row from the content region, not from the status bar
    // or by overdrawing the worktree strip.
    let mut cache = LayoutCache::default();
    let cfg = layout(24, 38, 80);
    cache.update(rect(200, 50), None, false, &cfg, 80);

    assert_eq!(cache.main_area.y, cache.wtbar_area.y + cache.wtbar_area.height);
    assert_eq!(
        cache.main_area.y + cache.main_area.height,
        cache.status_area.y,
        "main area must still butt up against the status bar"
    );
    // title(1) + menubar(1) + wtbar(1) + status(1) = 4 rows of chrome.
    assert_eq!(cache.main_area.height, 50 - 4);
}

#[test]
fn every_vertical_row_is_accounted_for() {
    // No gaps and no overlaps between the stacked regions, at a couple of
    // heights including one short enough to squeeze the main area.
    for h in [50_u16, 10, 6] {
        let mut cache = LayoutCache::default();
        cache.update(rect(120, h), None, false, &layout(24, 38, 80), 80);
        let total = cache.title_area.height
            + cache.menubar_area.height
            + cache.notif_area.height
            + cache.wtbar_area.height
            + cache.main_area.height
            + cache.status_area.height;
        assert_eq!(total, h, "regions must tile the frame exactly at height {h}");
    }
}
