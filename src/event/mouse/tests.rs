//! Unit tests for the mouse hit-testing geometry, double-click detection, and
//! left-margin click classification.

use super::explorer_panel::{diff_list_row_at, explorer_tree_row_at};
use super::viewer_panel::{
    MarginClickAction, MarginZone, classify_margin_click, thread_anchor_line,
};
use super::{ClickGeometry, Column, register_double_click, register_double_click_on};
use std::time::{Duration, Instant};

/// Build a `ClickGeometry` with the given column boundaries. Widths/heights
/// are set so that the `[<=>]` expand button (last 5 cols before each column
/// border, requiring width >= 7) is testable.
fn geom(left_end: u16, explorer_end: u16, viewer_end: u16) -> ClickGeometry {
    ClickGeometry {
        main_area: ratatui::layout::Rect::new(0, 1, viewer_end + 20, 40),
        left_w: left_end,
        explorer_w: explorer_end - left_end,
        viewer_w: viewer_end - explorer_end,
        left_end,
        explorer_end,
        viewer_end,
        explorer_mid_y: 20,
        terminal_claude_y: 1,
        terminal_split_y: 33,
    }
}

#[test]
fn gutter_and_badge_clicks_always_start_a_comment() {
    // The core of the overlap fix: a line inside an existing comment's
    // range (has_comment = true) must still start a NEW comment from the
    // number gutter and the "+" badge column — never get swallowed by
    // thread focus. The affordance is identical on every line.
    for zone in [MarginZone::NumberGutter, MarginZone::Badge] {
        for has_comment in [false, true] {
            assert_eq!(
                classify_margin_click(zone, has_comment, false, false),
                MarginClickAction::StartComment { extend: false }
            );
            assert_eq!(
                classify_margin_click(zone, has_comment, false, true),
                MarginClickAction::StartComment { extend: true }
            );
        }
    }
    // Even on a runnable-test line, the number gutter comments.
    assert_eq!(
        classify_margin_click(MarginZone::NumberGutter, false, true, false),
        MarginClickAction::StartComment { extend: false }
    );
}

#[test]
fn marker_click_focuses_existing_thread() {
    // The far-left 💬/│ marker column is the ONLY place thread focus lives.
    assert_eq!(
        classify_margin_click(MarginZone::Marker, true, false, false),
        MarginClickAction::ToggleThread
    );
    // Comment wins even on a commented test line (▶ lives in the badge
    // column, not here).
    assert_eq!(
        classify_margin_click(MarginZone::Marker, true, true, false),
        MarginClickAction::ToggleThread
    );
    // An empty marker cell falls back to starting a comment.
    assert_eq!(
        classify_margin_click(MarginZone::Marker, false, false, false),
        MarginClickAction::StartComment { extend: false }
    );
}

#[test]
fn badge_click_on_test_line_runs_the_test() {
    assert_eq!(
        classify_margin_click(MarginZone::Badge, false, true, false),
        MarginClickAction::RunTest
    );
    // A commented test line: ▶ still renders in the badge column, so the
    // click there still runs the test (thread focus is the marker's job).
    assert_eq!(
        classify_margin_click(MarginZone::Badge, true, true, false),
        MarginClickAction::RunTest
    );
}

#[test]
fn thread_anchor_redirects_mid_range_lines_to_nearest_end_line() {
    use crate::review_store::{Author, CommentKind, CommentStatus, ReviewComment};
    let range = |id: &str, start: u32, end: u32| ReviewComment {
        id: id.to_string(),
        worktree: "wt".to_string(),
        file_path: "src/main.rs".to_string(),
        line_start: start,
        line_end: Some(end),
        kind: CommentKind::Suggest,
        body: "body".to_string(),
        status: CommentStatus::Pending,
        commit_ref: "abc".to_string(),
        author: Author::User,
        branch: None,
        created_at: String::new(),
        updated_at: String::new(),
    };
    // Nested ranges L10-L20 and L11-L19: a mid-range │ click lands on the
    // nearest end line (💬), where both views render the thread.
    let both = [range("outer", 10, 20), range("inner", 11, 19)];
    assert_eq!(thread_anchor_line(&both, 15), 19);
    // A line covered only by the outer range anchors at its end.
    let outer_only = [range("outer", 10, 20)];
    assert_eq!(thread_anchor_line(&outer_only, 10), 20);
    // An end line anchors at itself.
    assert_eq!(thread_anchor_line(&both, 19), 19);
    assert_eq!(thread_anchor_line(&outer_only, 20), 20);
}

#[test]
fn divider_at_hits_both_cells_of_a_vertical_boundary() {
    use crate::app::Divider;
    // main_area spans y in [1, 41); a vertical boundary is a 2-cell zone at
    // {edge-1, edge}.
    let g = geom(0, 24, 62);
    assert_eq!(g.divider_at(23, 10), Some(Divider::ExplorerViewer));
    assert_eq!(g.divider_at(24, 10), Some(Divider::ExplorerViewer));
    assert_eq!(g.divider_at(61, 10), Some(Divider::ViewerTerminal));
    assert_eq!(g.divider_at(62, 10), Some(Divider::ViewerTerminal));
    // One cell either side of the zone is not a hit.
    assert_eq!(g.divider_at(22, 10), None);
    assert_eq!(g.divider_at(25, 10), None);
}

#[test]
fn divider_at_hits_horizontal_splits_within_their_column() {
    use crate::app::Divider;
    let g = geom(0, 24, 62); // explorer_mid_y=20, terminal_split_y=33
    // Explorer split: only inside the Explorer column [0, 24).
    assert_eq!(g.divider_at(10, 19), Some(Divider::ExplorerSplit));
    assert_eq!(g.divider_at(10, 20), Some(Divider::ExplorerSplit));
    assert_eq!(g.divider_at(10, 18), None);
    // The Explorer split does not extend into the Viewer/Terminal columns.
    assert_eq!(g.divider_at(40, 20), None);
    // Terminal split: only inside the Terminal column [62, right).
    assert_eq!(g.divider_at(70, 32), Some(Divider::TerminalSplit));
    assert_eq!(g.divider_at(70, 33), Some(Divider::TerminalSplit));
    assert_eq!(g.divider_at(40, 33), None);
}

#[test]
fn divider_at_vertical_boundary_wins_at_a_corner() {
    use crate::app::Divider;
    // (explorer_end-1, explorer_mid_y) is both a vertical-boundary cell and
    // on the Explorer split row — the vertical divider must take priority.
    let g = geom(0, 24, 62);
    assert_eq!(g.divider_at(23, 20), Some(Divider::ExplorerViewer));
}

#[test]
fn divider_at_misses_open_panel_area() {
    let g = geom(0, 24, 62);
    assert_eq!(g.divider_at(10, 10), None);
    assert_eq!(g.divider_at(70, 10), None);
}

#[test]
fn column_at_maps_columns_by_boundary() {
    let g = geom(20, 50, 90);
    assert_eq!(g.column_at(0), Column::Worktree);
    assert_eq!(g.column_at(19), Column::Worktree);
    assert_eq!(g.column_at(20), Column::Explorer);
    assert_eq!(g.column_at(49), Column::Explorer);
    assert_eq!(g.column_at(50), Column::Viewer);
    assert_eq!(g.column_at(89), Column::Viewer);
    assert_eq!(g.column_at(90), Column::Terminal);
    assert_eq!(g.column_at(200), Column::Terminal);
}

#[test]
fn expand_button_hits_last_cols_of_each_column() {
    use crate::app::Focus;
    // main_area.x == 0, so the worktree button spans [left_w-6, left_w-1).
    let g = geom(20, 50, 90);
    // Worktree button: cols 14..19.
    assert_eq!(g.expand_button_at(14), Some(Focus::Worktree));
    assert_eq!(g.expand_button_at(18), Some(Focus::Worktree));
    assert_eq!(g.expand_button_at(19), None); // btn_end is exclusive
    assert_eq!(g.expand_button_at(13), None);
    // Explorer button: [left_end + explorer_w - 6, ...) = [44, 49).
    assert_eq!(g.expand_button_at(44), Some(Focus::Explorer));
    assert_eq!(g.expand_button_at(48), Some(Focus::Explorer));
    // Viewer button: [explorer_end + viewer_w - 6, ...) = [84, 89).
    assert_eq!(g.expand_button_at(84), Some(Focus::Viewer));
    assert_eq!(g.expand_button_at(88), Some(Focus::Viewer));
}

#[test]
fn expand_button_absent_for_narrow_columns() {
    // A column narrower than 7 has no expand button.
    let g = geom(5, 50, 90);
    assert_eq!(g.expand_button_at(0), None);
    assert_eq!(g.expand_button_at(4), None);
}

#[test]
fn explorer_tree_row_at_rejects_the_border_row() {
    let g = geom(20, 50, 90); // main_area.y = 1, so the top border is row 1.
    assert_eq!(explorer_tree_row_at(&g, 0, 30, 1), None);
    // The first row inside the border resolves to visible index 0.
    assert_eq!(explorer_tree_row_at(&g, 0, 30, 2), Some(0));
}

#[test]
fn explorer_tree_row_at_rejects_columns_outside_the_explorer() {
    let g = geom(20, 50, 90);
    assert_eq!(explorer_tree_row_at(&g, 0, 19, 5), None); // Worktree column
    assert_eq!(explorer_tree_row_at(&g, 0, 50, 5), None); // Viewer column
    // The Explorer's own edge columns are still in-bounds.
    assert_eq!(explorer_tree_row_at(&g, 0, 20, 5), Some(3));
    assert_eq!(explorer_tree_row_at(&g, 0, 49, 5), Some(3));
}

#[test]
fn explorer_tree_row_at_rejects_the_bottom_half() {
    let g = geom(20, 50, 90); // explorer_mid_y = 20
    assert_eq!(explorer_tree_row_at(&g, 0, 30, 18), Some(16)); // last row actually drawn
    assert_eq!(explorer_tree_row_at(&g, 0, 30, 19), None); // the tree's own bottom border
    assert_eq!(explorer_tree_row_at(&g, 0, 30, 20), None); // Changed files starts here
    assert_eq!(explorer_tree_row_at(&g, 0, 30, 25), None);
}

/// Tie both hit-testers to the number of rows their panel actually renders,
/// derived the same way the renderers derive it (`height - 2` for the two
/// borders). A plain "row N maps to index M" assertion restates whatever the
/// function happens to do; this one fails the moment either panel accepts a
/// border row or drops a content row, which is exactly the class of bug that
/// let a click open a file that was never on screen.
#[test]
fn row_at_helpers_accept_exactly_the_rows_their_panel_draws() {
    let g = geom(20, 50, 90);
    let col = 30;

    let tree_inner = (g.explorer_mid_y - g.main_area.y) as usize - 2;
    let tree_hits: Vec<usize> = (0..g.explorer_mid_y)
        .filter_map(|row| explorer_tree_row_at(&g, 0, col, row))
        .collect();
    assert_eq!(tree_hits, (0..tree_inner).collect::<Vec<_>>());

    let diff_bottom = g.main_area.y + g.main_area.height;
    let diff_inner = (diff_bottom - g.explorer_mid_y) as usize - 2;
    let diff_hits: Vec<usize> = (g.explorer_mid_y..diff_bottom)
        .filter_map(|row| diff_list_row_at(&g, 0, 0, col, row))
        .collect();
    assert_eq!(diff_hits, (0..diff_inner).collect::<Vec<_>>());
}

#[test]
fn explorer_tree_row_at_adds_the_scroll_offset() {
    let g = geom(20, 50, 90);
    assert_eq!(explorer_tree_row_at(&g, 5, 30, 2), Some(5));
    assert_eq!(explorer_tree_row_at(&g, 5, 30, 7), Some(10));
}

#[test]
fn diff_list_row_at_rejects_the_top_half_and_its_border() {
    let g = geom(20, 50, 90); // explorer_mid_y = 20
    assert_eq!(diff_list_row_at(&g, 0, 0, 30, 19), None); // still the file tree
    assert_eq!(diff_list_row_at(&g, 0, 0, 30, 20), None); // the diff list's own top border
    assert_eq!(diff_list_row_at(&g, 0, 0, 30, 21), Some(0)); // first diff-list row
}

#[test]
fn diff_list_row_at_rejects_columns_outside_the_explorer() {
    let g = geom(20, 50, 90);
    assert_eq!(diff_list_row_at(&g, 0, 0, 19, 25), None); // Worktree column
    assert_eq!(diff_list_row_at(&g, 0, 0, 50, 25), None); // Viewer column
    assert_eq!(diff_list_row_at(&g, 0, 0, 20, 25), Some(4));
    assert_eq!(diff_list_row_at(&g, 0, 0, 49, 25), Some(4));
}

#[test]
fn diff_list_row_at_rejects_the_bottom_border() {
    // main_area = Rect::new(0, 1, .., 40) → bottom border row is 1 + 40 - 1 = 40,
    // which is where the "Ask Claude All" button lives, not a list row.
    let g = geom(20, 50, 90);
    assert_eq!(diff_list_row_at(&g, 0, 0, 30, 39), Some(18)); // last diff-list row
    assert_eq!(diff_list_row_at(&g, 0, 0, 30, 40), None);
    assert_eq!(diff_list_row_at(&g, 0, 0, 30, 45), None);
}

/// The error banner sits inside the list's inner area without occupying an
/// entry, so entries start that many rows lower. Getting this wrong lands the
/// click one file off and makes the banner itself open whatever is scrolled to
/// the top. Both the click handler and the hover tracker go through here, so
/// the offset only has to be right once.
#[test]
fn diff_list_row_at_skips_the_error_banner() {
    let g = geom(20, 50, 90); // explorer_mid_y = 20, first inner row = 21
    assert_eq!(diff_list_row_at(&g, 0, 2, 30, 21), None); // banner, not an entry
    assert_eq!(diff_list_row_at(&g, 0, 2, 30, 22), None);
    assert_eq!(diff_list_row_at(&g, 0, 2, 30, 23), Some(0)); // first real entry
    assert_eq!(diff_list_row_at(&g, 5, 2, 30, 23), Some(5)); // banner then scroll
}

#[test]
fn diff_list_row_at_adds_the_scroll_offset() {
    let g = geom(20, 50, 90);
    assert_eq!(diff_list_row_at(&g, 5, 0, 30, 21), Some(5));
    assert_eq!(diff_list_row_at(&g, 5, 0, 30, 26), Some(10));
}

#[test]
fn double_click_within_threshold() {
    let t0 = Instant::now();
    let mut last = t0;
    // A click 100ms after the previous one is a double-click.
    let is_double = register_double_click(&mut last, t0 + Duration::from_millis(100));
    assert!(is_double);
    assert_eq!(last, t0 + Duration::from_millis(100));
}

#[test]
fn single_click_beyond_threshold() {
    let t0 = Instant::now();
    let mut last = t0;
    // A click 400ms later is *not* a double-click (boundary is exclusive).
    assert!(!register_double_click(
        &mut last,
        t0 + Duration::from_millis(400)
    ));
    // And one well beyond the threshold is not either.
    let t1 = t0 + Duration::from_millis(400);
    assert!(!register_double_click(
        &mut last,
        t1 + Duration::from_millis(500)
    ));
}

#[test]
fn indexed_double_click_requires_same_idx() {
    let t0 = Instant::now();
    let mut last = t0;
    let mut last_idx = 0usize;
    // First click on idx 5: even within the time window, the stored idx (0)
    // differs, so it is not a double-click.
    let first =
        register_double_click_on(&mut last, &mut last_idx, 5, t0 + Duration::from_millis(50));
    assert!(!first);
    assert_eq!(last_idx, 5);
    // Second click on the same idx within the window: double-click.
    let second =
        register_double_click_on(&mut last, &mut last_idx, 5, t0 + Duration::from_millis(100));
    assert!(second);
}

#[test]
fn indexed_double_click_resets_on_different_idx() {
    let t0 = Instant::now();
    let mut last = t0;
    let mut last_idx = 3usize;
    // Quick click but on a different row → not a double-click, and the
    // stored index/time update so the next click compares against this one.
    let hit = register_double_click_on(&mut last, &mut last_idx, 7, t0 + Duration::from_millis(10));
    assert!(!hit);
    assert_eq!(last_idx, 7);
    assert_eq!(last, t0 + Duration::from_millis(10));
}
