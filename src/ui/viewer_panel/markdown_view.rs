//! Rendered-markdown mode of the viewer panel — the `.md`/`.markdown` file
//! shown as prose instead of source, plus the header Raw/Rendered toggle that
//! switches between the two.
//!
//! The prose is produced by the same renderer the SUMMARY pseudo-file uses
//! ([`crate::ui::markdown`], via `App::markdown_cache`), so headings, lists,
//! tables, and fenced code blocks look identical in both places.
//!
//! **This mode has no line numbers, and therefore no line-oriented features.**
//! Markdown rendering wraps, reflows, drops, and injects rows, so a screen row
//! no longer corresponds to a source line: gutter, line selection, hover
//! highlight, comment creation, inline comment threads, and line-anchored jumps
//! are all meaningless here and are switched off at their sources — see
//! [`crate::viewer::ViewerState::is_showing_rendered_markdown`], which every one
//! of them gates on. Notably, this renderer never writes
//! `content.screen_row_map`, which [`super::render`] clears on entry; that empty
//! map is what makes every mouse row lookup resolve to "no line".

use std::ops::Range;

use crate::app::App;
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{
    Block, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};

const RAW_LABEL: &str = "Raw";
const RENDERED_LABEL: &str = "Rendered";

/// Display width of the toggle chip `[Raw|Rendered]`.
const CHIP_W: u16 = 1 + RAW_LABEL.len() as u16 + 1 + RENDERED_LABEL.len() as u16 + 1;

/// Width the toggle claims in the title row: the chip plus one space of
/// separation from the `[<=>]` expand button to its right.
pub(crate) const TOGGLE_W: u16 = CHIP_W + 1;

/// Width of the `[<=>]` expand button the toggle is laid out against. Kept in
/// sync with `event::mouse::ClickGeometry::expand_button_at`, which owns that
/// button's own hit-test.
const EXPAND_BTN_W: u16 = 5;

/// Narrowest Viewer column that still gets a toggle. Below this the title would
/// have no room left, so the toggle is dropped entirely (keyboard and palette
/// still switch modes).
const MIN_VIEWER_W: u16 = TOGGLE_W + EXPAND_BTN_W + 8;

/// Screen columns of the toggle's two halves, in a Viewer column starting at
/// `viewer_x` and `viewer_w` wide.
pub(crate) struct ToggleSegments {
    /// Columns that select raw source (`[Raw`).
    pub raw: Range<u16>,
    /// Columns that select rendered markdown (`|Rendered]`).
    pub rendered: Range<u16>,
}

/// Where the header toggle sits, or `None` when the Viewer column is too narrow
/// to draw it.
///
/// The renderer ([`toggle_spans`], gated on this returning `Some`) and the mouse
/// hit-test in `event/mouse` both derive their layout from this one function, so
/// a toggle that isn't drawn can never be clicked, and a drawn one is always
/// clickable exactly where it appears.
///
/// The title line is right-aligned and ends one cell inside the block's right
/// border, laid out as `[Raw|Rendered] [<=>]`.
pub(crate) fn toggle_segments(viewer_x: u16, viewer_w: u16) -> Option<ToggleSegments> {
    if viewer_w < MIN_VIEWER_W {
        return None;
    }
    // Last drawable cell of the right-aligned title line, then step left past
    // the expand button and the separating space to the chip's own right edge.
    let line_end = viewer_x + viewer_w - 1; // exclusive
    let chip_end = line_end - EXPAND_BTN_W - 1; // exclusive
    let chip_start = chip_end - CHIP_W;
    // Split at the `|`: "[Raw" selects raw, "|Rendered]" selects rendered.
    let split = chip_start + 1 + RAW_LABEL.len() as u16;
    Some(ToggleSegments {
        raw: chip_start..split,
        rendered: split..chip_end,
    })
}

/// The toggle chip's spans, with the active mode highlighted, ready to be
/// appended to the Viewer's right-aligned title line. Caller must have checked
/// [`toggle_segments`] is `Some` for the current width.
pub(crate) fn toggle_spans(rendered: bool, theme: &Theme) -> Vec<Span<'static>> {
    let chrome = Style::default().fg(theme.muted);
    let active = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let inactive = Style::default().fg(theme.muted);
    vec![
        Span::styled("[", chrome),
        Span::styled(RAW_LABEL, if rendered { inactive } else { active }),
        Span::styled("|", chrome),
        Span::styled(RENDERED_LABEL, if rendered { active } else { inactive }),
        Span::styled("] ", chrome),
    ]
}

/// Render the open markdown file as prose, filling the whole panel.
///
/// `block` is the Viewer's own block (title + toggle already on it), so this
/// mode keeps the same frame as the raw view — only the contents change.
pub(super) fn render_markdown_view(frame: &mut Frame, area: Rect, app: &mut App, block: Block<'_>) {
    let inner_width = area.width.saturating_sub(2) as usize;
    let inner_height = area.height.saturating_sub(2) as usize;

    let (total, scroll, visible) = {
        let key = format!(
            "viewer-md:{}",
            app.viewer_state
                .content
                .current_file
                .as_deref()
                .unwrap_or("")
        );
        let body = app.viewer_state.content.file_content.join("\n");
        // Reserve a column on the right so wrapped prose never collides with
        // the scrollbar track (matching the summary view's inset).
        app.markdown_cache.render_window(
            &key,
            &body,
            inner_width.saturating_sub(1),
            &app.theme,
            &app.syntax_set,
            &app.syntect_theme,
            app.viewer_state.md_scroll,
            inner_height,
        )
    };

    // Record the total so the key handler can clamp scrolling, and write the
    // clamped scroll back so navigation stays responsive if the document shrank
    // (or re-wrapped shorter after the panel got wider).
    app.viewer_state.md_total_lines = total;
    app.viewer_state.md_scroll = scroll;

    frame.render_widget(ratatui::widgets::Clear, area);
    frame.render_widget(Paragraph::new(visible).block(block), area);

    if total > inner_height {
        let scrollbar_area = area.inner(Margin {
            horizontal: 0,
            vertical: 1,
        });
        let mut scrollbar_state =
            ScrollbarState::new(total.saturating_sub(inner_height)).position(scroll);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None);
        frame.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The chip must land immediately left of the `[<=>]` expand button, whose
    /// hit-test (`expand_button_at`) claims the 5 cells ending 2 before the
    /// column's right edge. Overlapping them would make one button eat the
    /// other's clicks.
    #[test]
    fn toggle_sits_just_left_of_the_expand_button() {
        let (x, w) = (40u16, 60u16);
        let seg = toggle_segments(x, w).expect("60 cols is plenty");
        let expand_start = x + w - 6;
        assert_eq!(
            seg.rendered.end + 1,
            expand_start,
            "one space must separate the chip from [<=>]"
        );
        assert!(seg.rendered.end <= expand_start);
    }

    #[test]
    fn toggle_halves_are_adjacent_and_correctly_sized() {
        let seg = toggle_segments(0, 80).unwrap();
        assert_eq!(seg.raw.end, seg.rendered.start, "no dead gap between halves");
        // "[Raw" and "|Rendered]".
        assert_eq!(seg.raw.end - seg.raw.start, 1 + RAW_LABEL.len() as u16);
        assert_eq!(
            seg.rendered.end - seg.rendered.start,
            1 + RENDERED_LABEL.len() as u16 + 1
        );
        assert_eq!(seg.rendered.end - seg.raw.start, CHIP_W);
    }

    /// A toggle that isn't drawn must not be clickable: both the renderer and
    /// the hit-test ask this same function, so `None` disables both at once.
    #[test]
    fn narrow_columns_get_no_toggle() {
        assert!(toggle_segments(0, MIN_VIEWER_W - 1).is_none());
        assert!(toggle_segments(0, 0).is_none());
        assert!(toggle_segments(0, MIN_VIEWER_W).is_some());
    }

    /// Whatever the column offset, the chip stays inside the panel.
    #[test]
    fn toggle_stays_within_the_column() {
        for w in [MIN_VIEWER_W, MIN_VIEWER_W + 1, 100, 300] {
            let seg = toggle_segments(7, w).unwrap();
            assert!(seg.raw.start > 7, "must not overlap the left border");
            assert!(seg.rendered.end < 7 + w, "must not overrun the right border");
        }
    }

    /// The decisive check: draw the header exactly as `file_view` does and
    /// confirm each cell `toggle_segments` claims really holds that part of the
    /// chip. Arithmetic agreement between the two is not enough — ratatui owns
    /// where a right-aligned title actually lands, and a drift there would send
    /// clicks to the wrong half (or into the `[<=>]` button) with nothing in the
    /// unit maths to show for it. Titles include a wide-glyph case, since a CJK
    /// filename costs 2 columns per character.
    #[test]
    fn drawn_columns_match_the_hit_test() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::layout::Alignment;
        use ratatui::style::Style;
        use ratatui::text::{Line, Span};
        use ratatui::widgets::{Block, Borders};

        let theme = Theme::default();
        for title in [" f.md ", " 設計メモ.md ", " a/very/deeply/nested/path/notes.md "] {
            for w in [MIN_VIEWER_W, MIN_VIEWER_W + 1, 40, 60, 120] {
                let mut term = Terminal::new(TestBackend::new(w, 5)).unwrap();
                // Same budget the renderer gives the title: borders + toggle +
                // `[<=>]` + one column of gap.
                let budget = (w as usize).saturating_sub(2 + TOGGLE_W as usize + 5 + 1);
                let fitted = crate::ui::viewer_panel::file_view::fit_title(title, budget);
                term.draw(|f| {
                    let mut spans = toggle_spans(false, &theme);
                    spans.push(Span::styled("[<=>]", Style::default()));
                    let block = Block::default()
                        .title(Span::raw(fitted.clone()))
                        .title_top(Line::from(spans).alignment(Alignment::Right))
                        .borders(Borders::ALL);
                    f.render_widget(block, Rect::new(0, 0, w, 5));
                })
                .unwrap();
                let buf = term.backend().buffer().clone();
                let seg = toggle_segments(0, w).expect("width is at least MIN_VIEWER_W");
                let cell = |x: u16| buf[(x, 0)].symbol().to_string();
                let ctx = format!("title={title:?} w={w}");

                assert_eq!(cell(seg.raw.start), "[", "{ctx}: raw half starts at '['");
                assert_eq!(
                    cell(seg.rendered.start),
                    "|",
                    "{ctx}: rendered half starts at the separator"
                );
                assert_eq!(
                    cell(seg.rendered.end - 1),
                    "]",
                    "{ctx}: rendered half ends at ']'"
                );
                // And the expand button really is where its own hit-test says.
                assert_eq!(cell(w - 6), "[", "{ctx}: [<=>] start");
                assert_eq!(cell(w - 2), "]", "{ctx}: [<=>] end");
            }
        }
    }

    #[test]
    fn spans_highlight_the_active_mode() {
        let theme = Theme::default();
        let raw_mode = toggle_spans(false, &theme);
        let rendered_mode = toggle_spans(true, &theme);
        // Index 1 is "Raw", index 3 is "Rendered".
        assert!(raw_mode[1].style.add_modifier.contains(Modifier::BOLD));
        assert!(!raw_mode[3].style.add_modifier.contains(Modifier::BOLD));
        assert!(!rendered_mode[1].style.add_modifier.contains(Modifier::BOLD));
        assert!(rendered_mode[3].style.add_modifier.contains(Modifier::BOLD));
        // The drawn width must match what the hit-test reserves.
        let drawn: usize = raw_mode
            .iter()
            .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
            .sum();
        assert_eq!(drawn as u16, TOGGLE_W);
    }
}
