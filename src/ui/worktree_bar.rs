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

/// Render the worktree monitor strip and record its clickable regions into
/// `app.wtbar_hits`.
pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    app.wtbar_hits.clear();
    if area.width == 0 || area.height == 0 {
        return;
    }

    let max_x = area.x + area.width;
    let mut x = area.x;
    let mut spans: Vec<Span> = Vec::new();
    let mut hits: Vec<WtbarHit> = Vec::new();

    // Identity marker so the strip reads as "worktrees".
    {
        let icon = "\u{2387} ";
        spans.push(Span::styled(
            icon,
            Style::default().fg(app.theme.muted).add_modifier(Modifier::BOLD),
        ));
        x += w(icon);
    }
    // New-worktree button.
    {
        let add = "[+] ";
        let aw = w(add);
        spans.push(Span::styled(
            add,
            Style::default()
                .fg(app.theme.success)
                .add_modifier(Modifier::BOLD),
        ));
        hits.push(WtbarHit {
            x0: x,
            x1: x + aw,
            action: WtbarAction::Add,
        });
        x += aw;
    }

    let total = app.worktrees.len();
    let mut shown = 0usize;
    // Reserve a little room on the right for the "+N" overflow hint.
    let reserve = 5u16;

    for (i, wt) in app.worktrees.iter().enumerate() {
        let waiting = app.terminal.cc_waiting_worktrees.contains(&wt.path);
        let active = app.terminal.cc_active_worktrees.contains(&wt.path);
        let is_current = i == app.selected_worktree;

        let mut chip = String::from(" ");
        if waiting {
            chip.push_str("\u{23f3} ");
        } else if active {
            chip.push_str("\u{25cf} ");
        }
        chip.push_str(&wt.branch);
        if !wt.is_clean {
            chip.push_str(&format!(" ~{}", wt.added + wt.modified + wt.deleted));
        }
        if let Some(a) = wt.ahead
            && a > 0
        {
            chip.push_str(&format!(" \u{2191}{a}"));
        }
        if let Some(b) = wt.behind
            && b > 0
        {
            chip.push_str(&format!(" \u{2193}{b}"));
        }
        chip.push(' ');

        let chip_w = w(&chip);
        let del = if wt.is_main { "" } else { "\u{2715} " };
        let del_w = w(del);
        let sep = if shown > 0 { "\u{2502} " } else { "" };
        let sep_w = w(sep);

        // Stop before we overrun (always keep room for the overflow hint).
        if shown > 0 && x + sep_w + chip_w + del_w + reserve > max_x {
            break;
        }

        if !sep.is_empty() {
            spans.push(Span::styled(sep, Style::default().fg(app.theme.border_secondary)));
            x += sep_w;
        }

        let chip_style = if is_current {
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD)
        } else if waiting {
            Style::default().fg(app.theme.warning)
        } else if active {
            Style::default().fg(app.theme.success)
        } else {
            Style::default().fg(app.theme.muted)
        };
        spans.push(Span::styled(chip.clone(), chip_style));
        hits.push(WtbarHit {
            x0: x,
            x1: x + chip_w,
            action: WtbarAction::Select(i),
        });
        x += chip_w;

        if !del.is_empty() {
            spans.push(Span::styled(del, Style::default().fg(app.theme.error)));
            hits.push(WtbarHit {
                x0: x,
                x1: x + del_w,
                action: WtbarAction::Delete(i),
            });
            x += del_w;
        }
        shown += 1;
    }

    if shown < total {
        spans.push(Span::styled(
            format!(" +{}", total - shown),
            Style::default().fg(app.theme.muted),
        ));
    }

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
