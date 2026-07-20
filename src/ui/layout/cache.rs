//! Cached per-frame layout rectangles and the accordion width calculation
//! they're built from.

use ratatui::layout::{Constraint, Layout, Rect};

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
    /// Explorer column width % used when computing this cache (cache key).
    pub explorer_width_pct: u16,
    /// Viewer column width % used when computing this cache (cache key).
    pub viewer_width_pct: u16,
    /// Claude Code area height % within the terminal column (cache key).
    pub terminal_split_pct: u16,
    /// File-tree height % within the Explorer column (cache key).
    pub explorer_split_pct: u16,
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
        layout: &crate::config::LayoutConfig,
        terminal_split_pct: u16,
    ) -> bool {
        if self.frame_area == frame_area
            && self.expanded_panel == expanded_panel
            && self.has_notifications == has_notifications
            && self.explorer_width_pct == layout.explorer_width_pct
            && self.viewer_width_pct == layout.viewer_width_pct
            && self.terminal_split_pct == terminal_split_pct
            && self.explorer_split_pct == layout.explorer_split_pct
        {
            return false;
        }

        self.frame_area = frame_area;
        self.expanded_panel = expanded_panel;
        self.has_notifications = has_notifications;
        self.explorer_width_pct = layout.explorer_width_pct;
        self.viewer_width_pct = layout.viewer_width_pct;
        // The terminal split is runtime-adjustable (grow/shrink shell), so it
        // comes in as a parameter rather than straight from the config.
        self.terminal_split_pct = terminal_split_pct;
        self.explorer_split_pct = layout.explorer_split_pct;

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

        let (left_w, explorer_w, viewer_w) = accordion_widths(
            expanded_panel,
            self.main_area.width,
            layout.explorer_width_pct,
            layout.viewer_width_pct,
        );
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
        let changed_files_pct = 100u16.saturating_sub(self.explorer_split_pct);
        let explorer_split = Layout::vertical([
            Constraint::Percentage(self.explorer_split_pct),
            Constraint::Percentage(changed_files_pct),
        ])
        .split(self.columns[1]);
        self.explorer_mid_y = explorer_split[1].y;

        // Terminal vertical split: Claude Code gets `terminal_split_pct`%,
        // shell gets the remainder.
        let shell_pct = 100u16.saturating_sub(terminal_split_pct);
        let terminal_split = Layout::vertical([
            Constraint::Percentage(terminal_split_pct),
            Constraint::Percentage(shell_pct),
        ])
        .split(self.columns[3]);
        self.terminal_split = [terminal_split[0], terminal_split[1]];

        true
    }
}

/// Calculate accordion panel widths based on panel expansion state.
///
/// Returns `(left_width, explorer_width, viewer_width)`. The right panel gets
/// whatever remains. `explorer_pct` and `viewer_pct` are the configured
/// percentages (0–100) used only in the default (non-maximized) layout.
pub(crate) fn accordion_widths(
    expanded_panel: Option<crate::app::Focus>,
    total_width: u16,
    explorer_pct: u16,
    viewer_pct: u16,
) -> (u16, u16, u16) {
    use crate::app::Focus;

    match expanded_panel {
        Some(Focus::Worktree) => (total_width, 0, 0),
        Some(Focus::Explorer) => (0, total_width, 0),
        Some(Focus::Viewer) => (0, 0, total_width),
        // The maximized editor takes the whole width via the explorer slot;
        // `render_ui` unions the explorer+viewer columns into one editor area,
        // so giving the explorer slot the full width (viewer 0) yields a
        // full-screen editor with the terminal column collapsed.
        Some(Focus::Editor) => (0, total_width, 0),
        Some(Focus::TerminalClaude | Focus::TerminalShell) => (0, 0, 0),
        None => {
            // Default proportions. The worktree column is gone (its status now
            // lives in the top strip), so it gets width 0 and the freed space
            // goes to the explorer and viewer review panes.
            let min_col = 3_u16;
            let explorer =
                ((total_width as u32 * explorer_pct as u32 / 100) as u16).max(min_col);
            let viewer = ((total_width as u32 * viewer_pct as u32 / 100) as u16).max(min_col);
            (0, explorer, viewer)
        }
    }
}
