//! Worktree status bar — a compact, full-width strip that replaces the old
//! left-hand worktree column.
//!
//! Shows every worktree at a glance (branch, dirty count, ahead/behind, and
//! Claude Code waiting/active state) so multiple parallel sessions can be
//! monitored peripherally. The strip is interactive: clicking a worktree jumps
//! to it (and its Claude session), `[+]` creates a worktree, and the per-chip
//! `✕` deletes one (with confirmation). The fuller list/detail UI lives in the
//! switcher modal (`render_switcher_overlay`).

use crate::app::App;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

/// What a clickable region of the worktree bar does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WtbarAction {
    /// Jump to the worktree at this index (and its Claude session).
    Select(usize),
    /// Delete the worktree at this index (with confirmation).
    Delete(usize),
    /// Create a new worktree.
    Add,
    /// Scroll the strip to reveal worktrees hidden off the left edge.
    ScrollLeft,
    /// Scroll the strip to reveal worktrees hidden off the right edge.
    ScrollRight,
}

/// A clickable region of the worktree bar, in absolute screen columns
/// (`x0` inclusive, `x1` exclusive) on the bar's single row.
#[derive(Clone, Copy, Debug)]
pub struct WtbarHit {
    pub x0: u16,
    pub x1: u16,
    pub action: WtbarAction,
}

fn w(s: &str) -> u16 {
    UnicodeWidthStr::width(s) as u16
}

/// Per-worktree data gathered before rendering, so the variable-width window
/// can be computed without holding a borrow on `app`.
struct Chip {
    text: String,
    width: u16,
    /// Delete button (`✕ `), empty for the main worktree.
    del: &'static str,
    del_width: u16,
    waiting: bool,
    active: bool,
    is_current: bool,
}

/// Compute the visible chip window `[start, end)` for the bar.
///
/// `slots[i]` is the full rendered width of chip `i` (chip + its delete button)
/// and `sep_w` the width of the separator drawn before every chip except the
/// first visible one. `avail` is the width left for chips and separators (the
/// caller has already reserved room for the overflow hints). `desired_start` is
/// the current scroll position; when `reveal` is set the window is panned the
/// minimum amount needed to include `selected`.
pub(crate) fn visible_window(
    slots: &[u16],
    sep_w: u16,
    avail: u16,
    desired_start: usize,
    selected: usize,
    reveal: bool,
) -> (usize, usize) {
    let total = slots.len();
    if total == 0 {
        return (0, 0);
    }

    // Greedily fill forward from `start`; always show at least one chip.
    let fill = |start: usize| -> usize {
        let mut used = 0u16;
        let mut end = start;
        while end < total {
            let extra = slots[end] + if end > start { sep_w } else { 0 };
            if used + extra > avail && end > start {
                break;
            }
            used = used.saturating_add(extra);
            end += 1;
        }
        end.max(start + 1).min(total)
    };

    // Smallest start that still reaches the last chip — clamps over-scrolling so
    // we never leave blank space on the right while chips stay hidden left.
    // A larger start can only push the window's end later or equal, so scanning
    // upward the first `start` that reaches the end is the smallest such start.
    let mut tail_start = 0;
    for s in 0..total {
        if fill(s) == total {
            tail_start = s;
            break;
        }
    }

    let mut start = desired_start.min(tail_start);

    if reveal {
        if selected < start {
            start = selected;
        } else {
            while start < selected && selected >= fill(start) {
                start += 1;
            }
        }
    }

    (start, fill(start))
}

/// Render the worktree monitor strip and record its clickable regions into
/// `app.wtbar_hits`.
pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    app.wtbar_hits.clear();
    if area.width == 0 || area.height == 0 {
        return;
    }

    let muted = app.theme.muted;
    let success = app.theme.success;
    let warning = app.theme.warning;
    let border = app.theme.border_secondary;
    let error = app.theme.error;

    // A smart/normal worktree is being created in the background → spin the
    // far-left marker so the activity is visible at a glance.
    let creating = app.worktree_mgr.pending_worktrees.iter().any(|p| {
        matches!(
            p.op,
            crate::app::PendingWorktreeOp::Creating | crate::app::PendingWorktreeOp::SmartCreating
        )
    });

    let max_x = area.x + area.width;
    let mut x = area.x;
    let mut spans: Vec<Span> = Vec::new();
    let mut hits: Vec<WtbarHit> = Vec::new();

    // Identity marker (spins while a worktree is being created).
    {
        let icon = if creating {
            format!("{} ", crate::ui::common::spinner_frame(app.ui_tick))
        } else {
            "\u{2387} ".to_string()
        };
        let style = if creating {
            Style::default().fg(success).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(muted).add_modifier(Modifier::BOLD)
        };
        x += w(&icon);
        spans.push(Span::styled(icon, style));
    }
    // The new-worktree button is pinned at the right edge (consistent with the
    // Claude/Shell session tab bars); reserve room for it here and render it at
    // the end. " [+]" = leading gap + button.
    let add = " [+]";
    let add_w = w(add);
    let chips_max_x = max_x.saturating_sub(add_w);

    // Gather chip data up front (releases the borrow on `app.worktrees`).
    let chips: Vec<Chip> = app
        .worktrees
        .iter()
        .enumerate()
        .map(|(i, wt)| {
            let waiting = app.terminal.cc_waiting_worktrees.contains(&wt.path);
            let active = app.terminal.cc_active_worktrees.contains(&wt.path);

            let mut text = String::from(" ");
            if waiting {
                text.push_str("\u{23f3} ");
            } else if active {
                text.push_str("\u{25cf} ");
            }
            text.push_str(&wt.branch);
            if !wt.is_clean {
                text.push_str(&format!(" ~{}", wt.added + wt.modified + wt.deleted));
            }
            if let Some(a) = wt.ahead
                && a > 0
            {
                text.push_str(&format!(" \u{2191}{a}"));
            }
            if let Some(b) = wt.behind
                && b > 0
            {
                text.push_str(&format!(" \u{2193}{b}"));
            }
            text.push(' ');

            // `[x]` to match the Claude/Shell session tabs (was `✕`); it sits
            // just past the chip's filled background, so the danger red stays
            // readable.
            let del = if wt.is_main { "" } else { "[x]" };
            Chip {
                width: w(&text),
                del,
                del_width: w(del),
                waiting,
                active,
                is_current: i == app.selected_worktree,
                text,
            }
        })
        .collect();

    let total = chips.len();
    // Separator drawn before every chip except the first visible one; its width
    // and the literal are defined together so they can't drift apart.
    let sep = "\u{2502} ";
    let sep_w = w(sep);
    let avail_full = chips_max_x.saturating_sub(x);

    // Does everything fit with no overflow hints? If so, skip the hint reserve.
    let slots: Vec<u16> = chips.iter().map(|c| c.width + c.del_width).collect();
    let all_fit = visible_window(&slots, sep_w, avail_full, 0, 0, false).1 == total;

    // Reserve room for the left/right overflow hints when scrolling is needed.
    let hint_reserve_per_side = 5u16;
    let avail = if all_fit {
        avail_full
    } else {
        avail_full.saturating_sub(hint_reserve_per_side * 2)
    };

    let (start, end) = if all_fit {
        (0, total)
    } else {
        visible_window(
            &slots,
            sep_w,
            avail,
            app.wtbar_scroll,
            app.selected_worktree,
            app.wtbar_reveal_selected,
        )
    };

    // Left overflow hint (clickable: scroll left). Tinted with warning if any
    // hidden worktree on that side is waiting for the user.
    if start > 0 {
        let waiting_left = chips[..start].iter().any(|c| c.waiting);
        let hint = format!("\u{2039}{} ", start);
        let hw = w(&hint);
        spans.push(Span::styled(
            hint,
            Style::default().fg(if waiting_left { warning } else { muted }),
        ));
        hits.push(WtbarHit {
            x0: x,
            x1: x + hw,
            action: WtbarAction::ScrollLeft,
        });
        x += hw;
    }

    for (offset, chip) in chips[start..end].iter().enumerate() {
        let i = start + offset;
        if offset > 0 {
            spans.push(Span::styled(sep, Style::default().fg(border)));
            x += sep_w;
        }

        let chip_style = if chip.is_current {
            // Filled chip so the active worktree reads at a glance, not just a
            // color shift — the chip text carries its own surrounding spaces.
            Style::default()
                .fg(app.theme.selected_fg)
                .bg(app.theme.selected_bg)
                .add_modifier(Modifier::BOLD)
        } else if chip.waiting {
            Style::default().fg(warning)
        } else if chip.active {
            Style::default().fg(success)
        } else {
            Style::default().fg(muted)
        };
        spans.push(Span::styled(chip.text.clone(), chip_style));
        hits.push(WtbarHit {
            x0: x,
            x1: x + chip.width,
            action: WtbarAction::Select(i),
        });
        x += chip.width;

        if !chip.del.is_empty() {
            spans.push(Span::styled(chip.del, Style::default().fg(error)));
            hits.push(WtbarHit {
                x0: x,
                x1: x + chip.del_width,
                action: WtbarAction::Delete(i),
            });
            x += chip.del_width;
        }
    }

    // Right overflow hint (clickable: scroll right).
    if end < total {
        let waiting_right = chips[end..].iter().any(|c| c.waiting);
        let hint = format!(" {}\u{203a}", total - end);
        let hw = w(&hint);
        spans.push(Span::styled(
            hint,
            Style::default().fg(if waiting_right { warning } else { muted }),
        ));
        hits.push(WtbarHit {
            x0: x,
            x1: x + hw,
            action: WtbarAction::ScrollRight,
        });
        x += hw;
    }

    // Pin the new-worktree [+] button flush against the right edge.
    if x < chips_max_x {
        let pad = (chips_max_x - x) as usize;
        spans.push(Span::raw(" ".repeat(pad)));
        x = chips_max_x;
    }
    spans.push(Span::styled(
        add,
        Style::default().fg(success).add_modifier(Modifier::BOLD),
    ));
    hits.push(WtbarHit {
        x0: x,
        x1: x + add_w,
        action: WtbarAction::Add,
    });

    app.wtbar_scroll = start;
    app.wtbar_reveal_selected = false;
    app.wtbar_hits = hits;
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Render the worktree switcher modal: a centered popup that reuses the full
/// worktree panel (list + detail + sessions) so selection/creation/etc. keep
/// their existing UI and key handling.
pub fn render_switcher_overlay(frame: &mut Frame, area: Rect, app: &mut App) {
    // Clamp lower bounds to `area` so a tiny terminal can't make min > max
    // (which would panic in `u16::clamp`).
    let w = ((area.width as u32 * 60 / 100) as u16).clamp(24.min(area.width), area.width);
    let h = ((area.height as u32 * 70 / 100) as u16).clamp(6.min(area.height), area.height);
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let popup = Rect::new(x, y, w, h);
    frame.render_widget(ratatui::widgets::Clear, popup);
    crate::ui::worktree_panel::render(frame, popup, app);
}

#[cfg(test)]
mod tests {
    use super::visible_window;

    // Ten uniform-width chips, separator width 1.
    const W: &[u16] = &[10, 10, 10, 10, 10, 10, 10, 10, 10, 10];

    #[test]
    fn everything_fits_when_avail_is_large() {
        // Plenty of room → full window, no panning.
        let (start, end) = visible_window(W, 1, 1000, 0, 0, false);
        assert_eq!((start, end), (0, 10));
    }

    #[test]
    fn greedy_fill_stops_at_available_width() {
        // avail 32: chip(10) + sep1+chip(10) + sep1+chip(10) = 32 → 3 chips.
        let (start, end) = visible_window(W, 1, 32, 0, 0, false);
        assert_eq!((start, end), (0, 3));
    }

    #[test]
    fn at_least_one_chip_even_when_too_narrow() {
        let (start, end) = visible_window(W, 1, 4, 0, 0, false);
        assert_eq!((start, end), (0, 1));
    }

    #[test]
    fn scroll_offset_moves_the_window() {
        let (start, end) = visible_window(W, 1, 32, 4, 4, false);
        assert_eq!((start, end), (4, 7));
    }

    #[test]
    fn over_scroll_is_clamped_to_keep_right_edge_full() {
        // Desired start 9 would waste space; clamp so the last chip sits at the
        // right edge (window of 3 ending at the tail).
        let (start, end) = visible_window(W, 1, 32, 9, 0, false);
        assert_eq!((start, end), (7, 10));
    }

    #[test]
    fn reveal_pans_left_when_selected_is_before_window() {
        // Window currently at [4,7); select chip 1 → pan back so it's visible.
        let (start, end) = visible_window(W, 1, 32, 4, 1, true);
        assert_eq!(start, 1);
        assert!((start..end).contains(&1));
    }

    #[test]
    fn reveal_pans_right_when_selected_is_after_window() {
        // Window at [0,3); select chip 8 → advance until it's visible.
        let (start, end) = visible_window(W, 1, 32, 0, 8, true);
        assert!((start..end).contains(&8));
    }

    #[test]
    fn reveal_does_not_move_when_selected_already_visible() {
        let (start, end) = visible_window(W, 1, 32, 3, 4, true);
        assert_eq!((start, end), (3, 6));
    }

    #[test]
    fn empty_list_is_handled() {
        assert_eq!(visible_window(&[], 1, 100, 0, 0, true), (0, 0));
    }
}
