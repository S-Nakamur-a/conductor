//! tmux-style panel resizing: keyboard-driven divider moves, mouse-drag divider
//! tracking, and the clamped percentage math shared by both input paths.

use super::App;

/// Direction of a tmux-style pane resize, relative to the focused panel.
///
/// Semantics mirror tmux `resize-pane -L/-R/-U/-D`: the focused panel grows
/// toward the given direction by moving the divider it shares with the neighbor
/// on that side. When the panel has no neighbor on that side (it sits against
/// the edge), the opposite divider moves instead, so the panel shrinks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeDir {
    Left,
    Right,
    Up,
    Down,
}

/// A panel boundary that can be grabbed and dragged with the mouse to resize.
///
/// Each variant maps onto the same clamped state mutator the keyboard
/// (Ctrl+Alt+Arrow) resize drives, so mouse and keyboard share one source of
/// truth for the layout ratios ([`App::drag_divider_to`] resolves the mapping).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Divider {
    /// Vertical boundary between the Explorer and Viewer columns.
    ExplorerViewer,
    /// Vertical boundary between the Viewer and Terminal columns.
    ViewerTerminal,
    /// Horizontal boundary between the Explorer's file tree and changed-files list.
    ExplorerSplit,
    /// Horizontal boundary between the Claude and Shell terminal panes.
    TerminalSplit,
}

impl App {
    pub(super) fn cmd_toggle_panel_expand(&mut self) {
        if self.expanded_panel == Some(self.focus) {
            self.expanded_panel = None;
        } else {
            self.expanded_panel = Some(self.focus);
        }
    }

    /// Step (percentage points) each horizontal pane resize moves a column
    /// divider.
    const RESIZE_STEP_PCT: u16 = 5;
    /// Minimum width percentage for each of the three columns (Explorer, Viewer,
    /// Terminal), so a tmux-style resize can never collapse a column to nothing.
    const MIN_COL_PCT: u16 = 10;
    /// Step (percentage points) each vertical pane resize moves the Claude/Shell
    /// divider.
    const TERMINAL_SPLIT_STEP: u16 = 5;
    /// Bounds for the runtime Claude-area percentage, leaving at least this much
    /// for each of the two terminal panes so neither can vanish. `pub(super)`
    /// because `app::appearance` also clamps the live-reloaded config value
    /// against these same bounds.
    pub(super) const TERMINAL_SPLIT_MIN: u16 = 20;
    pub(super) const TERMINAL_SPLIT_MAX: u16 = 80;

    /// Resize the focused panel tmux-style, growing it toward `dir`.
    ///
    /// Maps the focused panel and direction onto one of the three adjustable
    /// dividers (Explorer|Viewer, Viewer|Terminal, Claude|Shell). The focused
    /// panel grows toward `dir` by moving the divider it shares with its
    /// neighbor on that side; against an edge it moves the only divider it has,
    /// shrinking instead — mirroring `resize-pane -L/-R/-U/-D`. The middle
    /// (Viewer) column can therefore push both of its borders, so it never
    /// becomes the cramped pane that can only shrink.
    pub fn resize_focused_pane(&mut self, dir: ResizeDir) {
        use super::focus::Focus;
        let step = Self::RESIZE_STEP_PCT as i16;
        let changed = match dir {
            ResizeDir::Left | ResizeDir::Right => {
                let grow_right = matches!(dir, ResizeDir::Right);
                match self.focus {
                    // The worktree strip is full-width, not one of the three
                    // resizable columns — nothing to resize from there.
                    Focus::Worktree => false,
                    // Leftmost column: left/right ride the Explorer|Viewer divider.
                    Focus::Explorer => {
                        self.move_explorer_viewer_divider(if grow_right { step } else { -step })
                    }
                    // Middle column pushes whichever border faces `dir`.
                    Focus::Viewer => {
                        if grow_right {
                            self.move_viewer_terminal_divider(step)
                        } else {
                            self.move_explorer_viewer_divider(-step)
                        }
                    }
                    // Rightmost column: left grows it (shrinks Viewer), right shrinks it.
                    Focus::TerminalClaude | Focus::TerminalShell | Focus::Editor => {
                        self.move_viewer_terminal_divider(if grow_right { step } else { -step })
                    }
                }
            }
            ResizeDir::Up | ResizeDir::Down => {
                // Two columns have a vertical split: the terminal (Claude/Shell)
                // and the Explorer (file tree / changed files). Down grows the
                // top pane, Up shrinks it.
                let down = matches!(dir, ResizeDir::Down);
                match self.focus {
                    Focus::TerminalClaude | Focus::TerminalShell => {
                        let step = Self::TERMINAL_SPLIT_STEP as i16;
                        self.adjust_terminal_split(if down { step } else { -step })
                    }
                    Focus::Explorer => {
                        let step = Self::TERMINAL_SPLIT_STEP as i16;
                        self.adjust_explorer_split(if down { step } else { -step })
                    }
                    _ => false,
                }
            }
        };
        // Persist once per keypress (only when a ratio actually moved — a resize
        // that hit the clamp floor writes nothing). The mouse-drag path persists
        // once on release instead, so both share the same clamped mutators
        // without writing the config on every intermediate step.
        if changed {
            self.persist_layout();
        }
    }

    /// Drag `divider` so its boundary tracks the mouse at screen cell
    /// (`col`, `row`), reusing the same clamped mutators as the keyboard resize.
    /// Returns whether a ratio actually moved. Does **not** persist — the caller
    /// writes the config once when the drag ends, avoiding a disk write per
    /// mouse event.
    pub fn drag_divider_to(&mut self, divider: Divider, col: u16, row: u16) -> bool {
        // Snapshot the (Copy) geometry before taking `&mut self` for the mutator.
        let main = self.layout_cache.main_area;
        let explorer_col = self.layout_cache.columns[1];
        let terminal_col = self.layout_cache.columns[3];
        match divider {
            // Vertical dividers: percentages are relative to the main area width.
            Divider::ExplorerViewer => {
                if main.width == 0 {
                    return false;
                }
                let target_px = col.saturating_sub(main.x);
                let target_pct = (target_px as u32 * 100 / main.width as u32) as i16;
                let delta = target_pct - self.config.layout.explorer_width_pct as i16;
                self.move_explorer_viewer_divider(delta)
            }
            Divider::ViewerTerminal => {
                if main.width == 0 {
                    return false;
                }
                // The divider sits at the right edge of (Explorer + Viewer); with
                // Explorer fixed, the target Viewer % is that combined width minus
                // Explorer.
                let combined_px = col.saturating_sub(main.x);
                let combined_pct = (combined_px as u32 * 100 / main.width as u32) as i16;
                let target_v = combined_pct - self.config.layout.explorer_width_pct as i16;
                let delta = target_v - self.config.layout.viewer_width_pct as i16;
                self.move_viewer_terminal_divider(delta)
            }
            // Horizontal dividers: percentages are relative to their column height.
            Divider::ExplorerSplit => {
                if explorer_col.height == 0 {
                    return false;
                }
                let target_px = row.saturating_sub(explorer_col.y);
                let target_pct = (target_px as u32 * 100 / explorer_col.height as u32) as i16;
                let delta = target_pct - self.config.layout.explorer_split_pct as i16;
                self.adjust_explorer_split(delta)
            }
            Divider::TerminalSplit => {
                if terminal_col.height == 0 {
                    return false;
                }
                let target_px = row.saturating_sub(terminal_col.y);
                let target_pct = (target_px as u32 * 100 / terminal_col.height as u32) as i16;
                let delta = target_pct - self.terminal_split_pct as i16;
                self.adjust_terminal_split(delta)
            }
        }
    }

    /// Move the Explorer|Viewer divider by `delta` points (positive = rightward,
    /// enlarging Explorer and shrinking Viewer). Terminal width is conserved.
    /// Clamped so neither Explorer nor Viewer drops below [`Self::MIN_COL_PCT`].
    /// Returns whether the ratio changed; the caller persists.
    fn move_explorer_viewer_divider(&mut self, delta: i16) -> bool {
        let (new_e, new_v) = clamp_ev_divider(
            self.config.layout.explorer_width_pct,
            self.config.layout.viewer_width_pct,
            delta,
            Self::MIN_COL_PCT,
        );
        if new_e == self.config.layout.explorer_width_pct {
            return false;
        }
        self.config.layout.explorer_width_pct = new_e;
        self.config.layout.viewer_width_pct = new_v;
        self.after_horizontal_resize();
        true
    }

    /// Move the Viewer|Terminal divider by `delta` points (positive = rightward,
    /// enlarging Viewer and shrinking Terminal). Explorer width is unchanged.
    /// Clamped so neither Viewer nor Terminal drops below [`Self::MIN_COL_PCT`].
    /// Returns whether the ratio changed; the caller persists.
    fn move_viewer_terminal_divider(&mut self, delta: i16) -> bool {
        let new_v = clamp_vt_divider(
            self.config.layout.explorer_width_pct,
            self.config.layout.viewer_width_pct,
            delta,
            Self::MIN_COL_PCT,
        );
        if new_v == self.config.layout.viewer_width_pct {
            return false;
        }
        self.config.layout.viewer_width_pct = new_v;
        self.after_horizontal_resize();
        true
    }

    /// Shared tail for a column resize: redraw and flash the new split. Persist
    /// is left to the caller (keyboard: per keypress; mouse: on drag release).
    fn after_horizontal_resize(&mut self) {
        self.dirty.mark_all();
        let e = self.config.layout.explorer_width_pct;
        let v = self.config.layout.viewer_width_pct;
        let t = 100u16.saturating_sub(e.saturating_add(v));
        self.set_status_info(format!("Layout: Explorer {e}% / Viewer {v}% / Terminal {t}%"));
    }

    /// Adjust the runtime Claude-area height percentage by `delta` points,
    /// clamped so both the Claude and Shell panes keep a usable minimum. A
    /// positive `delta` enlarges the Claude pane (shrinks the Shell); negative
    /// enlarges the Shell. Flashes the resulting split. Returns whether the ratio
    /// changed; the caller persists.
    fn adjust_terminal_split(&mut self, delta: i16) -> bool {
        let next = (self.terminal_split_pct as i16 + delta)
            .clamp(Self::TERMINAL_SPLIT_MIN as i16, Self::TERMINAL_SPLIT_MAX as i16)
            as u16;
        if next == self.terminal_split_pct {
            return false;
        }
        self.terminal_split_pct = next;
        // Keep the in-memory config in sync so the appearance snapshot matches
        // what we write on persist — that makes the config watcher's reload a
        // no-op (it only reacts when the snapshot differs), avoiding a self-write
        // loop.
        self.config.layout.terminal_split_pct = next;
        self.dirty.mark_all();
        self.set_status_info(format!(
            "Terminal split: Claude {next}% / Shell {}%",
            100 - next
        ));
        true
    }

    /// Adjust the Explorer column's file-tree height percentage by `delta`
    /// points (positive grows the file tree, shrinking the changed-files list),
    /// clamped so both panels keep a usable minimum. Flashes. Returns whether the
    /// ratio changed; the caller persists.
    fn adjust_explorer_split(&mut self, delta: i16) -> bool {
        let next = (self.config.layout.explorer_split_pct as i16 + delta)
            .clamp(Self::TERMINAL_SPLIT_MIN as i16, Self::TERMINAL_SPLIT_MAX as i16)
            as u16;
        if next == self.config.layout.explorer_split_pct {
            return false;
        }
        self.config.layout.explorer_split_pct = next;
        self.dirty.mark_all();
        self.set_status_info(format!(
            "Explorer split: tree {next}% / changed files {}%",
            100 - next
        ));
        true
    }

    /// Persist the current panel proportions to `config.toml`. Best-effort: a
    /// write failure is logged, never fatal (the in-memory layout still applies).
    pub(crate) fn persist_layout(&self) {
        if let Err(e) = crate::config::persist_layout_proportions(
            self.config.layout.explorer_width_pct,
            self.config.layout.viewer_width_pct,
            self.terminal_split_pct,
            self.config.layout.explorer_split_pct,
        ) {
            log::warn!("failed to persist layout proportions: {e}");
        }
    }
}

/// Compute the new `(explorer, viewer)` width percentages after moving the
/// Explorer|Viewer divider by `delta` points. Explorer+Viewer is conserved
/// (Terminal width is untouched), and both columns are kept `>= min`. A `delta`
/// that would push a column below the floor is clamped, so the divider stops at
/// the boundary rather than overshooting.
fn clamp_ev_divider(explorer: u16, viewer: u16, delta: i16, min: u16) -> (u16, u16) {
    let e = explorer as i16;
    let v = viewer as i16;
    let min = min as i16;
    let upper = (e + v - min).max(min);
    let new_e = (e + delta).clamp(min, upper);
    (new_e as u16, (e + v - new_e) as u16)
}

/// Compute the new Viewer width percentage after moving the Viewer|Terminal
/// divider by `delta` points. Explorer is untouched; Viewer and the implicit
/// Terminal column (`100 - explorer - viewer`) are each kept `>= min`.
fn clamp_vt_divider(explorer: u16, viewer: u16, delta: i16, min: u16) -> u16 {
    let e = explorer as i16;
    let v = viewer as i16;
    let min = min as i16;
    // Terminal = 100 - E - V, so keep new V in [min, 100 - E - min].
    let upper = (100 - e - min).max(min);
    (v + delta).clamp(min, upper) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── tmux-style pane resize divider math ──────────────────────────

    const MIN: u16 = 10;

    #[test]
    fn ev_divider_moves_space_between_explorer_and_viewer() {
        // Growing Explorer (delta +5) takes 5 points from Viewer; Terminal
        // (the conserved remainder) is untouched.
        assert_eq!(clamp_ev_divider(24, 38, 5, MIN), (29, 33));
        // Growing Viewer (delta -5) gives 5 points back to Viewer.
        assert_eq!(clamp_ev_divider(24, 38, -5, MIN), (19, 43));
        // Explorer + Viewer is always conserved.
        let (e, v) = clamp_ev_divider(24, 38, 5, MIN);
        assert_eq!(e + v, 62);
    }

    #[test]
    fn ev_divider_clamps_at_min_floor() {
        // Explorer can't drop below MIN even with a big shrink.
        assert_eq!(clamp_ev_divider(12, 50, -5, MIN), (10, 52));
        // Viewer can't drop below MIN even when Explorer wants to grow.
        assert_eq!(clamp_ev_divider(50, 12, 5, MIN), (52, 10));
    }

    #[test]
    fn vt_divider_protects_the_terminal_column() {
        // Explorer 24, Viewer 38 → Terminal 38. Growing Viewer right eats into
        // Terminal but never past its MIN floor: max Viewer = 100 - 24 - 10 = 66.
        assert_eq!(clamp_vt_divider(24, 38, 5, MIN), 43);
        assert_eq!(clamp_vt_divider(24, 64, 5, MIN), 66); // clamped, Terminal=10
        // Shrinking Viewer (grow Terminal) is floored at Viewer = MIN.
        assert_eq!(clamp_vt_divider(24, 12, -5, MIN), 10);
    }

    #[test]
    fn dividers_never_let_a_column_vanish() {
        // Sweep deltas across the full range; all three columns stay >= MIN.
        for delta in [-50i16, -20, -5, 5, 20, 50] {
            let (e, v) = clamp_ev_divider(24, 38, delta, MIN);
            let t = 100u16.saturating_sub(e + v);
            assert!(e >= MIN && v >= MIN && t >= MIN, "ev delta={delta}: {e}/{v}/{t}");

            let v2 = clamp_vt_divider(24, 38, delta, MIN);
            let t2 = 100u16.saturating_sub(24 + v2);
            assert!(v2 >= MIN && t2 >= MIN, "vt delta={delta}: 24/{v2}/{t2}");
        }
    }
}
