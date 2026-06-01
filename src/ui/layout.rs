//! Layout rendering — top-level UI orchestration and overlay helpers.
//!
//! Contains the main `render_ui` function that composes all panels and overlays,
//! plus the `accordion_widths` helper for calculating column proportions.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};

use crate::app::App;

/// Cached layout rectangles computed once per frame.
/// Shared between render_ui, mouse event handler, PTY sizing, and decoration.
#[derive(Default, Clone)]
pub struct LayoutCache {
    /// Frame area used to compute this cache (cache key).
    pub frame_area: Rect,
    /// Expanded panel state when cache was computed (cache key).
    pub expanded_panel: Option<crate::app::Focus>,
    /// Whether notification bar was visible (cache key).
    pub has_notifications: bool,
    /// Title bar area.
    pub title_area: Rect,
    /// Notification bar area.
    pub notif_area: Rect,
    /// Worktree monitor strip area (full-width, between notif and main).
    pub wtbar_area: Rect,
    /// Main content area (between title and status bars).
    pub main_area: Rect,
    /// Status bar area.
    pub status_area: Rect,
    /// Column areas: [worktree, explorer, viewer, terminal].
    pub columns: [Rect; 4],
    /// Explorer panel vertical split mid-point Y coordinate.
    pub explorer_mid_y: u16,
    /// Terminal split: [claude_area, shell_area].
    pub terminal_split: [Rect; 2],
}

impl LayoutCache {
    /// Recompute layout if inputs changed. Returns true if cache was updated.
    pub fn update(
        &mut self,
        frame_area: Rect,
        expanded_panel: Option<crate::app::Focus>,
        has_notifications: bool,
    ) -> bool {
        if self.frame_area == frame_area
            && self.expanded_panel == expanded_panel
            && self.has_notifications == has_notifications
        {
            return false;
        }

        self.frame_area = frame_area;
        self.expanded_panel = expanded_panel;
        self.has_notifications = has_notifications;

        // The notification bar is gone — Claude-waiting state is now shown by
        // the worktree strip (the waiting worktree is highlighted there).
        let notif_height: u16 = 0;
        // The worktree monitor strip is hidden while a panel is maximized, to
        // give the expanded panel the full height.
        let wtbar_height: u16 = if expanded_panel.is_some() { 0 } else { 1 };

        let outer = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(notif_height),
            Constraint::Length(wtbar_height),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(frame_area);

        self.title_area = outer[0];
        self.notif_area = outer[1];
        self.wtbar_area = outer[2];
        self.main_area = outer[3];
        self.status_area = outer[4];

        let (left_w, explorer_w, viewer_w) = accordion_widths(expanded_panel, self.main_area.width);
        let right_w = self
            .main_area
            .width
            .saturating_sub(left_w.saturating_add(explorer_w).saturating_add(viewer_w));

        let cols = Layout::horizontal([
            Constraint::Length(left_w),
            Constraint::Length(explorer_w),
            Constraint::Length(viewer_w),
            Constraint::Length(right_w),
        ])
        .split(self.main_area);

        self.columns = [cols[0], cols[1], cols[2], cols[3]];

        // Explorer 50/50 vertical split
        let explorer_split =
            Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(self.columns[1]);
        self.explorer_mid_y = explorer_split[1].y;

        // Terminal 80/20 vertical split
        let terminal_split =
            Layout::vertical([Constraint::Percentage(80), Constraint::Percentage(20)])
                .split(self.columns[3]);
        self.terminal_split = [terminal_split[0], terminal_split[1]];

        true
    }
}

/// Calculate accordion panel widths based on panel expansion state.
///
/// Returns `(left_width, explorer_width, viewer_width)`. The right panel gets whatever remains.
pub(crate) fn accordion_widths(
    expanded_panel: Option<crate::app::Focus>,
    total_width: u16,
) -> (u16, u16, u16) {
    use crate::app::Focus;

    match expanded_panel {
        Some(Focus::Worktree) => (total_width, 0, 0),
        Some(Focus::Explorer) => (0, total_width, 0),
        Some(Focus::Viewer) => (0, 0, total_width),
        Some(Focus::TerminalClaude | Focus::TerminalShell) => (0, 0, 0),
        None => {
            // Default proportions. The worktree column is gone (its status now
            // lives in the top strip), so it gets width 0 and the freed space
            // goes to the explorer and viewer review panes.
            let min_col = 3_u16;
            let explorer = ((total_width as u32 * 24 / 100) as u16).max(min_col);
            let viewer = ((total_width as u32 * 38 / 100) as u16).max(min_col);
            (0, explorer, viewer)
        }
    }
}

/// Top-level UI renderer — 3-column accordion layout + status bar.
pub(crate) fn render_ui(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let has_notifications = !app.terminal.cc_waiting_worktrees.is_empty();

    // Update layout cache (no-op if nothing changed).
    app.layout_cache
        .update(area, app.expanded_panel, has_notifications);

    let title_area = app.layout_cache.title_area;
    let wtbar_area = app.layout_cache.wtbar_area;
    let main_area = app.layout_cache.main_area;
    let status_area = app.layout_cache.status_area;

    // ── Title bar ───────────────────────────────────────────────────
    super::common::render_title_bar(frame, title_area, app);

    // ── Worktree monitor strip (replaces the old left column and the
    //    former CC-waiting notification bar) ─────────────────────────
    super::worktree_bar::render(frame, wtbar_area, app);

    // ── Accordion column widths (from cache) ───────────────────────
    let columns = app.layout_cache.columns;

    // ── Focused-panel surface ───────────────────────────────────────
    // Lift the focused list panel (worktree / explorer) out of its
    // neighbours with a subtle surface fill, so the active column reads
    // at a glance in peripheral vision. Painted before the panels so
    // their (bg-transparent) content draws on top; viewer is left alone
    // as a reading pane and the terminal keeps its own PTY background.
    let focused_surface_col = match app.focus {
        crate::app::Focus::Explorer => Some(1),
        _ => None,
    };
    if let Some(col) = focused_surface_col {
        let fill = ratatui::widgets::Block::default()
            .style(ratatui::style::Style::default().bg(app.theme.panel_focused_bg));
        frame.render_widget(fill, columns[col]);
    }

    // ── Column 0 (worktree) is gone — its status is in the top strip. ──

    // ── Column 1: Explorer (file tree + diff list) ──────────────────
    super::explorer_panel::render(frame, columns[1], app);

    // ── Column 2: Viewer (file content) ─────────────────────────────
    if app.viewer_state.is_current_file_media()
        && let Some(ref rel_path) = app.viewer_state.content.current_file.clone()
    {
        let full_path = app.selected_worktree_path().join(rel_path);
        let cols = columns[2].width;
        let rows = columns[2].height;
        app.viewer_state
            .media_state
            .render_if_needed(&full_path, rel_path, cols, rows);
    }
    super::viewer_panel::render(frame, columns[2], app);

    // ── Column 3: Terminal split (Claude 80% / Shell 20%) ───────────
    let terminal_split = app.layout_cache.terminal_split;

    super::terminal_claude::render(frame, terminal_split[0], app);
    super::terminal_shell::render(frame, terminal_split[1], app);

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
        super::panel_overlay::render_panel_overlay(frame, app);
    }

    // ── Overlays ────────────────────────────────────────────────────
    // These render on top of everything else when active.

    // Worktree input mode overlays (not part of ActiveOverlay enum).
    match app.worktree_mgr.input_mode {
        crate::app::WorktreeInputMode::CreatingWorktree => {
            super::dashboard::render_worktree_input_overlay(frame, main_area, app);
        }
        crate::app::WorktreeInputMode::CreatingWorktreeBase => {
            super::dashboard::render_worktree_base_input_overlay(frame, main_area, app);
        }
        crate::app::WorktreeInputMode::ConfirmingDelete => {
            render_confirming_delete_overlay(frame, main_area, app);
        }
        crate::app::WorktreeInputMode::ConfirmingDeleteBranch => {
            super::dashboard::render_delete_branch_confirm_overlay(frame, main_area, app);
        }
        crate::app::WorktreeInputMode::ConfirmingUngrab => {
            render_confirm_overlay(frame, main_area, app, " Confirm Ungrab ", app.theme.warning);
        }
        crate::app::WorktreeInputMode::ConfirmingReset => {
            render_confirm_overlay(frame, main_area, app, " Confirm Reset ", app.theme.error);
        }
        crate::app::WorktreeInputMode::SmartDescription => {
            super::dashboard::render_smart_description_overlay(frame, main_area, app);
        }
        crate::app::WorktreeInputMode::Normal => {}
    }
    // Review input overlays (not part of ActiveOverlay enum).
    if app.review_state.input_mode != crate::review_state::ReviewInputMode::Normal {
        super::review::render_input_overlay(frame, main_area, app);
    }
    if app.review_state.template_picker_active {
        super::review::render_template_picker_overlay(
            frame,
            main_area,
            &app.review_state,
            &app.theme,
        );
    }
    if app.review_state.comment_detail_active {
        super::review::render_comment_detail_overlay(frame, main_area, app);
    }
    // ActiveOverlay-based overlays.
    match app.overlays.active {
        crate::overlay::ActiveOverlay::None => {}
        crate::overlay::ActiveOverlay::History => {
            super::dashboard::render_history_overlay(frame, main_area, app);
        }
        crate::overlay::ActiveOverlay::CherryPick => {
            super::dashboard::render_cherry_pick_overlay(frame, main_area, app);
        }
        crate::overlay::ActiveOverlay::SwitchBranch => {
            super::dashboard::render_switch_branch_overlay(frame, main_area, app);
        }
        crate::overlay::ActiveOverlay::Grab => {
            super::dashboard::render_grab_overlay(frame, main_area, app);
        }
        crate::overlay::ActiveOverlay::Prune => {
            super::dashboard::render_prune_overlay(frame, main_area, app);
        }
        crate::overlay::ActiveOverlay::RepoSelector => {
            super::dashboard::render_repo_selector_overlay(frame, main_area, app);
        }
        crate::overlay::ActiveOverlay::OpenRepo => {
            super::dashboard::render_open_repo_overlay(frame, main_area, app);
        }
        crate::overlay::ActiveOverlay::ResumeSession => {
            super::dashboard::render_resume_session_overlay(frame, main_area, app);
        }
        crate::overlay::ActiveOverlay::GrepSearch => {
            super::grep_search::render_grep_search_overlay(frame, main_area, app);
        }
        crate::overlay::ActiveOverlay::CommandPalette => {
            super::dashboard::render_command_palette_overlay(frame, main_area, app);
        }
        crate::overlay::ActiveOverlay::Help => {
            super::dashboard::render_help_overlay(frame, main_area, app);
        }
        crate::overlay::ActiveOverlay::WorktreeSwitcher => {
            super::worktree_bar::render_switcher_overlay(frame, main_area, app);
        }
        crate::overlay::ActiveOverlay::CommentList => {
            super::explorer_panel::render_comment_list_overlay(frame, main_area, app);
        }
    }
    // Fuzzy filename-search ("jump to file") modal — rendered at the top level
    // so it works even when the explorer column is collapsed (viewer maximized).
    if app.viewer_state.filename_search.filename_search_active {
        super::dashboard::render_filename_search_overlay(frame, main_area, app);
    }
    match app.update_state {
        crate::app::UpdateState::Confirming => {
            super::dashboard::render_update_confirm_overlay(frame, main_area, app);
        }
        crate::app::UpdateState::InProgress
        | crate::app::UpdateState::Restarting
        | crate::app::UpdateState::Failed => {
            super::dashboard::render_update_progress_overlay(frame, main_area, app);
        }
        crate::app::UpdateState::Idle => {}
    }

    // ── References overlay (panel-level, not part of OverlayManager) ──
    if app.references_overlay.active {
        crate::ui::references::render_references_overlay(frame, main_area, app);
    }

    // ── Symbol action overlay (after hint selection) ──
    if app.symbol_action_overlay.active {
        crate::ui::symbol_action::render_symbol_action_overlay(frame, main_area, app);
    }

    // ── Skip reason modal ────────────────────────────────────────────
    if let Some(ref reason) = app.worktree_mgr.skip_reason {
        render_skip_reason_overlay(frame, main_area, reason, &app.theme);
    }

    // ── Status bar ──────────────────────────────────────────────────
    // Show worktree branch + repo on the right of status bar.
    let _worktree_branch = app
        .worktrees
        .get(app.selected_worktree)
        .map(|w| w.branch.as_str())
        .unwrap_or("");
    super::common::render_status_bar(frame, status_area, app);
    super::common::render_worktree_label(
        frame,
        status_area,
        _worktree_branch,
        &app.repo_path,
        &app.theme,
    );

    // ── Party mode (hidden) ──────────────────────────────────────────
    // Post-process the finished frame so rainbow borders, a shimmering
    // title bar, and confetti land on top of everything (including overlays).
    if app.party_mode {
        super::party::apply_party_effects(frame, app);
    }
}

/// Render a small confirmation overlay for worktree deletion.
fn render_confirming_delete_overlay(frame: &mut Frame, area: Rect, app: &App) {
    render_confirm_overlay(frame, area, app, " Confirm Delete ", app.theme.error);
}

/// Generic small confirmation overlay with a customizable title and border color.
fn render_confirm_overlay(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    title: &str,
    border_color: ratatui::style::Color,
) {
    if let Some(ref status_msg) = app.status_message {
        let msg = &status_msg.text;
        let popup_height = 3_u16;
        let popup_width = area.width.saturating_sub(8).min(60);
        let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
        let y = area.y + area.height.saturating_sub(popup_height + 2);
        let popup_area = Rect::new(x, y, popup_width, popup_height);

        frame.render_widget(ratatui::widgets::Clear, popup_area);

        let block = ratatui::widgets::Block::default()
            .title(title)
            .borders(ratatui::widgets::Borders::ALL)
            .border_style(ratatui::style::Style::default().fg(border_color));

        let inner = block.inner(popup_area);
        frame.render_widget(block, popup_area);

        let paragraph = ratatui::widgets::Paragraph::new(ratatui::text::Span::styled(
            msg.as_str(),
            ratatui::style::Style::default().fg(app.theme.fg),
        ));
        frame.render_widget(paragraph, inner);
    }
}

/// Render a skip-reason informational popup.
fn render_skip_reason_overlay(
    frame: &mut Frame,
    area: Rect,
    reason: &str,
    theme: &crate::theme::Theme,
) {
    let popup_height = 5_u16;
    let popup_width = area.width.saturating_sub(8).min(60);
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let block = ratatui::widgets::Block::default()
        .title(" Skipped ")
        .borders(ratatui::widgets::Borders::ALL)
        .border_style(ratatui::style::Style::default().fg(theme.warning));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let text = vec![
        ratatui::text::Line::from(ratatui::text::Span::styled(
            reason,
            ratatui::style::Style::default().fg(theme.warning),
        )),
        ratatui::text::Line::from(ratatui::text::Span::styled(
            "(Esc) 閉じる",
            ratatui::style::Style::default().fg(theme.muted),
        )),
    ];
    let paragraph =
        ratatui::widgets::Paragraph::new(text).wrap(ratatui::widgets::Wrap { trim: true });
    frame.render_widget(paragraph, inner);
}
