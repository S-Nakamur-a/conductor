//! Top-level frame renderer: composes the 3-column accordion, the status/title
//! bars, and the resize-divider highlight.

use ratatui::Frame;
use ratatui::layout::Rect;

use crate::app::App;

use super::overlays::render_overlays;

/// Top-level UI renderer — 3-column accordion layout + status bar.
pub(crate) fn render_ui(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let has_notifications = !app.terminal.cc_waiting_worktrees.is_empty();

    // Update layout cache (no-op if nothing changed).
    // Disjoint field borrows: `app.layout_cache` mutably, `app.config.layout`
    // immutably — Rust allows this because they are separate struct fields.
    app.layout_cache.update(
        area,
        app.expanded_panel,
        has_notifications,
        &app.config.layout,
        app.terminal_split_pct,
    );

    let title_area = app.layout_cache.title_area;
    let wtbar_area = app.layout_cache.wtbar_area;
    let main_area = app.layout_cache.main_area;
    let status_area = app.layout_cache.status_area;

    // ── Title bar ───────────────────────────────────────────────────
    super::super::common::render_title_bar(frame, title_area, app);

    // ── Worktree monitor strip (replaces the old left column and the
    //    former CC-waiting notification bar) ─────────────────────────
    super::super::worktree_bar::render(frame, wtbar_area, app);

    // ── Accordion column widths (from cache) ───────────────────────
    let columns = app.layout_cache.columns;

    // ── Column 0 (worktree) is gone — its status is in the top strip. ──

    if app.editor.is_some() {
        // The embedded editor replaces the Explorer + Viewer columns with one
        // merged PTY panel (the terminal column stays put). When maximized,
        // accordion_widths gives the explorer slot the full width and the
        // viewer slot zero, so the union below is the whole main area.
        let region = Rect {
            x: columns[1].x,
            y: columns[1].y,
            width: columns[1].width.saturating_add(columns[2].width),
            height: columns[1].height,
        };
        super::super::editor_panel::render(frame, region, app);
    } else {
        // ── Column 1: Explorer (file tree + diff list) ──────────────────
        super::super::explorer_panel::render(frame, columns[1], app);

        // ── Column 2: Viewer (file content) ─────────────────────────────
        if app.viewer_state.is_current_file_media()
            && let Some(ref rel_path) = app.viewer_state.content.current_file.clone()
        {
            let full_path = app.selected_worktree_path().join(rel_path);
            let cols = columns[2].width;
            let rows = columns[2].height;
            // Tier B: pixel-quality rendering via the graphics protocol.
            let picker = if app.rich_tier.has_graphics() {
                app.rich_picker
            } else {
                None
            };
            app.viewer_state
                .media_state
                .render_if_needed(&full_path, rel_path, cols, rows, picker);
        }
        super::super::viewer_panel::render(frame, columns[2], app);
    }

    // ── Column 3: Terminal split (Claude 80% / Shell 20%) ───────────
    let terminal_split = app.layout_cache.terminal_split;
    super::super::terminal_claude::render(frame, terminal_split[0], app);
    super::super::terminal_shell::render(frame, terminal_split[1], app);

    // ── Resize affordance: light up a hovered/dragged divider ────────
    highlight_active_divider(frame, app);

    // ── Panel number overlay (Alt+/ toggle) ──────────────────────────
    // Only show when no other overlay/modal is active.
    if app.show_panel_overlay()
        && app.overlays.active == crate::overlay::ActiveOverlay::None
        && app.worktree_mgr.input_mode == crate::app::WorktreeInputMode::Normal
        && app.review_state.input_mode == crate::review_state::ReviewInputMode::Normal
        && app.update_state == crate::app::UpdateState::Idle
        && !app.review_state.comment_detail_active
        && app.worktree_mgr.skip_reason.is_none()
    {
        super::super::panel_overlay::render_panel_overlay(frame, app);
    }

    render_overlays(frame, main_area, app);

    // ── Status bar ──────────────────────────────────────────────────
    // Show worktree branch + repo on the right of status bar.
    let _worktree_branch = app
        .worktrees
        .get(app.selected_worktree)
        .map(|w| w.branch.as_str())
        .unwrap_or("");
    super::super::common::render_status_bar(frame, status_area, app);
    super::super::common::render_worktree_label(
        frame,
        status_area,
        _worktree_branch,
        &app.repo_path,
        &app.theme,
    );

    // ── Rich mode (Tier A) ───────────────────────────────────────────
    // Post-process the finished frame with the gradient breathing border
    // and Claude-waiting glow. Skipped while party mode is active: party
    // finds the focused border by colour equality with `border_focused`,
    // which the gradient would break.
    if app.rich_tier.is_rich() && !app.party_mode {
        super::super::rich::apply_rich_effects(frame, app);
    }

    // ── Party mode (hidden) ──────────────────────────────────────────
    // Post-process the finished frame so rainbow borders, a shimmering
    // title bar, and confetti land on top of everything (including overlays).
    if app.party_mode {
        super::super::party::apply_party_effects(frame, app);
    }
}

/// Paint the divider currently hovered or being dragged in the theme accent
/// colour — the terminal stand-in for the `col-resize`/`row-resize` cursor a GUI
/// would show, since crossterm can't switch the OS cursor shape. A live drag
/// wins over hover and keeps the boundary lit even if the cursor slips a cell
/// off it mid-drag. Only border glyphs are recoloured, so panel content is never
/// touched. Runs before the rich/party post-processing, which only recolours
/// cells matching the focused-border colour and so leaves the accent line alone.
fn highlight_active_divider(frame: &mut Frame, app: &App) {
    use crate::app::Divider;

    let Some(divider) = app.divider_drag.or(app.divider_hover) else {
        return;
    };
    let lc = &app.layout_cache;
    let color = app.theme.accent;

    // Resolve the divider to (is_vertical, fixed coordinate, span area). The
    // fixed coordinate is the top/left panel's border cell (`edge - 1`), which
    // is the visible divider line.
    let (vertical, fixed, area) = match divider {
        Divider::ExplorerViewer => {
            let edge = lc.columns[1].x.saturating_add(lc.columns[1].width);
            (true, edge.saturating_sub(1), lc.main_area)
        }
        Divider::ViewerTerminal => {
            let edge = lc.columns[2].x.saturating_add(lc.columns[2].width);
            (true, edge.saturating_sub(1), lc.main_area)
        }
        Divider::ExplorerSplit => (false, lc.explorer_mid_y.saturating_sub(1), lc.columns[1]),
        Divider::TerminalSplit => {
            (false, lc.terminal_split[1].y.saturating_sub(1), lc.columns[3])
        }
    };

    let buf = frame.buffer_mut();
    if vertical {
        if fixed < area.x || fixed >= area.x.saturating_add(area.width) {
            return;
        }
        for y in area.y..area.y.saturating_add(area.height) {
            if let Some(cell) = buf.cell_mut((fixed, y))
                && super::super::party::is_border_glyph(cell.symbol())
            {
                cell.set_fg(color);
            }
        }
    } else {
        if fixed < area.y || fixed >= area.y.saturating_add(area.height) {
            return;
        }
        for x in area.x..area.x.saturating_add(area.width) {
            if let Some(cell) = buf.cell_mut((x, fixed))
                && super::super::party::is_border_glyph(cell.symbol())
            {
                cell.set_fg(color);
            }
        }
    }
}
