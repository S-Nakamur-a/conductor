//! Shared scrolling tab bar for the Claude / Shell terminal panels.
//!
//! The bare `ratatui::widgets::Tabs` widget renders every tab left-to-right and
//! silently clips whatever runs past the right edge — so with enough sessions
//! the all-important `[+]` "new session" button disappears. This module renders
//! a tab bar where:
//!
//! * the `[+]` (new) and expand toggle are **pinned** to the right edge and are
//!   therefore always visible and always clickable, and
//! * the session tabs scroll horizontally in the space that remains, with
//!   `‹N` / `N›` overflow hints (mirroring `worktree_bar`'s strip), the active
//!   tab auto-revealed.
//!
//! Rendering records clickable regions (absolute screen columns) so mouse
//! handling consults the exact same geometry instead of re-deriving widths.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::theme::Theme;
use crate::ui::worktree_bar::visible_window;

fn w(s: &str) -> u16 {
    UnicodeWidthStr::width(s) as u16
}

/// Truncate `s` to at most `max_w` display columns, appending `…` when cut.
/// Width-aware so it never splits a wide (CJK) glyph across the boundary.
fn truncate_to_width(s: &str, max_w: u16) -> String {
    use unicode_width::UnicodeWidthChar;
    let max_w = max_w as usize;
    if UnicodeWidthStr::width(s) <= max_w {
        return s.to_string();
    }
    let budget = max_w.saturating_sub(1); // reserve a column for the ellipsis
    let mut out = String::new();
    let mut acc = 0usize;
    for ch in s.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if acc + cw > budget {
            break;
        }
        out.push(ch);
        acc += cw;
    }
    out.push('\u{2026}');
    out
}

/// What a clickable region of the tab bar does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabAction {
    /// Switch to the session with this global PTY index.
    Select(usize),
    /// Close the session with this global PTY index.
    Close(usize),
    /// Spawn a new session.
    Add,
    /// Toggle panel expansion.
    Expand,
    /// Scroll the tab strip left (reveal tabs hidden off the left edge).
    ScrollLeft,
    /// Scroll the tab strip right (reveal tabs hidden off the right edge).
    ScrollRight,
}

/// A clickable region of the tab bar, in absolute screen columns
/// (`x0` inclusive, `x1` exclusive) on the bar's single row.
#[derive(Clone, Copy, Debug)]
pub struct TabHit {
    pub x0: u16,
    pub x1: u16,
    pub action: TabAction,
}

/// One session tab to render.
pub struct TabItem {
    /// Global PTY session index (what `Select`/`Close` carry).
    pub global_idx: usize,
    /// Pre-formatted label, e.g. `"[CC:🎹]"`.
    pub label: String,
    /// Whether this is the active session.
    pub is_active: bool,
    /// Base style for the label of an *inactive* tab (waiting pulse, etc.).
    /// Ignored for the active tab, which uses the strong selection fill.
    pub label_style: Style,
}

/// Render the tab bar and return its clickable regions plus the resolved scroll
/// position (the index of the first visible tab, to be stored back into state).
///
/// `scroll` is the desired first-visible tab index; `reveal` pans the window the
/// minimum needed to keep the active tab visible (set it the frame after the
/// active session changes).
pub fn render(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    items: &[TabItem],
    scroll: usize,
    reveal: bool,
    is_expanded: bool,
) -> (Vec<TabHit>, usize) {
    let mut hits: Vec<TabHit> = Vec::new();
    if area.width == 0 || area.height == 0 {
        return (hits, scroll);
    }

    let max_x = area.x + area.width;

    // ── Pinned right cluster: [+] and the expand toggle, always visible. ──
    let add = "[+]";
    let (expand_label, expand_color) = if is_expanded {
        ("[>=<]", theme.border_focused)
    } else {
        ("[<=>]", theme.border_unfocused)
    };
    // " [+] [<=>]" — leading space separates the cluster from the tabs.
    let right_w = 1 + w(add) + 1 + w(expand_label);
    let tabs_region_w = area.width.saturating_sub(right_w);

    let mut spans: Vec<Span> = Vec::new();
    let mut x = area.x;

    // Active tab index (for reveal) and per-tab slot widths.
    let selected = items.iter().position(|t| t.is_active).unwrap_or(0);
    let close = " [x]";
    let close_w = w(close);
    let sep = " ";
    let sep_w = w(sep);
    let total = items.len();
    let hint_reserve_per_side = 4u16; // "‹NN " / " NN›"

    // Cap each label so even one very long session name can't overrun the
    // scroll region and shove the pinned [+]/expand cluster off-screen — that
    // was the "long name hides the new/close buttons" bug. A sane upper bound
    // also keeps several tabs visible at once; truncated labels end with "…".
    let max_label_w = tabs_region_w
        .saturating_sub(close_w + sep_w + hint_reserve_per_side * 2)
        .clamp(4, 28);
    let labels: Vec<String> = items
        .iter()
        .map(|t| truncate_to_width(&t.label, max_label_w))
        .collect();
    let slots: Vec<u16> = labels.iter().map(|l| w(l) + close_w).collect();

    // Does everything fit without hints? If so, skip the hint reserve.
    let all_fit = visible_window(&slots, sep_w, tabs_region_w, 0, 0, false).1 == total;
    let avail = if all_fit {
        tabs_region_w
    } else {
        tabs_region_w.saturating_sub(hint_reserve_per_side * 2)
    };
    let (start, end) = if all_fit {
        (0, total)
    } else {
        visible_window(&slots, sep_w, avail, scroll, selected, reveal)
    };

    // Left overflow hint (clickable: scroll left).
    if start > 0 {
        let hint = format!("\u{2039}{} ", start);
        let hw = w(&hint);
        spans.push(Span::styled(hint, Style::default().fg(theme.muted)));
        hits.push(TabHit {
            x0: x,
            x1: x + hw,
            action: TabAction::ScrollLeft,
        });
        x += hw;
    }

    for (offset, item) in items[start..end].iter().enumerate() {
        let label = &labels[start + offset];
        if offset > 0 {
            spans.push(Span::raw(sep));
            x += sep_w;
        }
        let label_w = w(label);
        if item.is_active {
            // Strong filled tab so the active session reads at a glance. The
            // [x] is left OUTSIDE the fill (on the default background) so its
            // danger-red stays readable — filling it would put red on the accent
            // bg with poor contrast. Matches the worktree bar's chip + [x].
            let fill = Style::default()
                .fg(theme.selected_fg)
                .bg(theme.selected_bg)
                .add_modifier(Modifier::BOLD);
            spans.push(Span::styled(label.clone(), fill));
            spans.push(Span::styled(close, Style::default().fg(theme.error)));
        } else {
            spans.push(Span::styled(label.clone(), item.label_style));
            spans.push(Span::styled(close, Style::default().fg(theme.muted)));
        }
        // Select covers the label (+ leading space of the close suffix);
        // Close covers the "[x]" glyphs only.
        hits.push(TabHit {
            x0: x,
            x1: x + label_w + 1,
            action: TabAction::Select(item.global_idx),
        });
        hits.push(TabHit {
            x0: x + label_w + 1,
            x1: x + label_w + close_w,
            action: TabAction::Close(item.global_idx),
        });
        x += label_w + close_w;
    }

    // Right overflow hint (clickable: scroll right), before the pinned cluster.
    if end < total {
        let hint = format!(" {}\u{203a}", total - end);
        let hw = w(&hint);
        spans.push(Span::styled(hint, Style::default().fg(theme.muted)));
        hits.push(TabHit {
            x0: x,
            x1: x + hw,
            action: TabAction::ScrollRight,
        });
        x += hw;
    }

    // Pad so the pinned cluster sits flush against the right edge.
    let cluster_x = max_x.saturating_sub(right_w);
    if x < cluster_x {
        let pad = (cluster_x - x) as usize;
        spans.push(Span::raw(" ".repeat(pad)));
        x = cluster_x;
    }

    // Pinned [+] (new session).
    spans.push(Span::raw(sep));
    x += sep_w;
    spans.push(Span::styled(
        add,
        Style::default()
            .fg(theme.success)
            .add_modifier(Modifier::BOLD),
    ));
    hits.push(TabHit {
        x0: x,
        x1: x + w(add),
        action: TabAction::Add,
    });
    x += w(add);

    // Pinned expand toggle.
    spans.push(Span::raw(sep));
    x += sep_w;
    spans.push(Span::styled(expand_label, Style::default().fg(expand_color)));
    hits.push(TabHit {
        x0: x,
        x1: x + w(expand_label),
        action: TabAction::Expand,
    });

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
    (hits, start)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn items(n: usize) -> Vec<TabItem> {
        (0..n)
            .map(|i| TabItem {
                global_idx: i,
                label: format!("[CC:{i}]"),
                is_active: i == 0,
                label_style: Style::default(),
            })
            .collect()
    }

    fn render_hits(width: u16, items: &[TabItem], scroll: usize) -> Vec<TabHit> {
        let theme = Theme::default();
        let mut terminal = Terminal::new(TestBackend::new(width, 1)).unwrap();
        let mut captured = Vec::new();
        terminal
            .draw(|f| {
                let area = f.area();
                let (hits, _) = render(f, area, &theme, items, scroll, false, false);
                captured = hits;
            })
            .unwrap();
        captured
    }

    #[test]
    fn add_and_expand_are_always_hittable_even_when_tabs_overflow() {
        // Far more tabs than fit in a narrow bar — the [+] button used to be the
        // first thing clipped. It must remain present and clickable.
        let hits = render_hits(30, &items(20), 0);
        assert!(
            hits.iter().any(|h| h.action == TabAction::Add),
            "the [+] new-session button must always be hittable"
        );
        assert!(hits.iter().any(|h| h.action == TabAction::Expand));
    }

    #[test]
    fn overflow_exposes_scroll_affordances() {
        let hits = render_hits(24, &items(20), 5);
        assert!(hits.iter().any(|h| h.action == TabAction::ScrollLeft));
        assert!(hits.iter().any(|h| h.action == TabAction::ScrollRight));
    }

    #[test]
    fn pinned_cluster_sits_flush_against_the_right_edge() {
        let width = 40u16;
        let hits = render_hits(width, &items(3), 0);
        let rightmost = hits.iter().map(|h| h.x1).max().unwrap();
        assert_eq!(rightmost, width, "expand toggle should end at the right edge");
    }

    #[test]
    fn one_very_long_label_still_keeps_add_and_expand_pinned() {
        // A single session with a huge name used to overrun and hide [+]/[x].
        let items = vec![TabItem {
            global_idx: 0,
            label: "[CC:a-really-extremely-long-session-name-that-overflows]".to_string(),
            is_active: true,
            label_style: Style::default(),
        }];
        let width = 30u16;
        let hits = render_hits(width, &items, 0);
        assert!(hits.iter().any(|h| h.action == TabAction::Add));
        assert!(hits.iter().any(|h| h.action == TabAction::Expand));
        // Nothing may extend past the bar's right edge.
        assert!(hits.iter().all(|h| h.x1 <= width));
        // The single tab is still selectable.
        assert!(hits.iter().any(|h| h.action == TabAction::Select(0)));
    }

    #[test]
    fn all_tabs_hittable_when_they_fit() {
        let hits = render_hits(80, &items(3), 0);
        for i in 0..3 {
            assert!(
                hits.iter()
                    .any(|h| h.action == TabAction::Select(i)),
                "tab {i} should be selectable when everything fits"
            );
        }
    }
}
