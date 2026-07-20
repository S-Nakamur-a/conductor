//! Generic `Span`/`Line` manipulation helpers shared by the file and diff
//! views: horizontal scroll clipping, underline/hint-label overlays, and
//! digit-width calculation for the gutter.

use crate::theme::Theme;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

/// Skip `offset` characters from the beginning of a sequence of `Span`s and
/// truncate to at most `max_width` characters, preserving per-span styling.
pub(super) fn h_scroll_spans(
    spans: Vec<Span<'static>>,
    offset: usize,
    max_width: usize,
) -> Vec<Span<'static>> {
    let mut remaining_skip = offset;
    let mut remaining_width = max_width;
    let mut result: Vec<Span<'static>> = Vec::new();
    for span in spans {
        if remaining_width == 0 {
            break;
        }
        let char_count = span.content.chars().count();
        // Left clipping: skip characters for horizontal scroll offset.
        if remaining_skip > 0 {
            if remaining_skip >= char_count {
                remaining_skip -= char_count;
                continue;
            }
            let s: String = span.content.chars().skip(remaining_skip).collect();
            let len = s.chars().count();
            if len <= remaining_width {
                remaining_width -= len;
                result.push(Span::styled(s, span.style));
            } else {
                let truncated: String = s.chars().take(remaining_width).collect();
                remaining_width = 0;
                result.push(Span::styled(truncated, span.style));
            }
            remaining_skip = 0;
        } else {
            // Right clipping: truncate to remaining panel width.
            if char_count <= remaining_width {
                remaining_width -= char_count;
                result.push(span);
            } else {
                let truncated: String = span.content.chars().take(remaining_width).collect();
                remaining_width = 0;
                result.push(Span::styled(truncated, span.style));
            }
        }
    }
    result
}

pub(super) fn digit_count(n: usize) -> usize {
    if n == 0 {
        return 1;
    }
    let mut count = 0;
    let mut val = n;
    while val > 0 {
        count += 1;
        val /= 10;
    }
    count
}

/// Apply underline + accent fg to spans within `[start_col..end_col)` of the original content.
/// `h_scroll` is the horizontal scroll offset already applied to the spans.
pub(super) fn apply_underline_range(
    spans: Vec<Span<'static>>,
    start_col: usize,
    end_col: usize,
    h_scroll: usize,
    accent: Color,
) -> Vec<Span<'static>> {
    // Convert original content cols to visible cols (after h_scroll).
    let vis_start = start_col.saturating_sub(h_scroll);
    let vis_end = end_col.saturating_sub(h_scroll);
    if vis_start >= vis_end {
        return spans;
    }

    let mut result = Vec::with_capacity(spans.len() + 4);
    let mut pos: usize = 0;
    for span in spans {
        let span_len = span.content.chars().count();
        let span_end = pos + span_len;

        if span_end <= vis_start || pos >= vis_end {
            // Entirely outside the underline range.
            result.push(span);
        } else {
            // This span overlaps the underline range.
            let rel_start = vis_start.saturating_sub(pos);
            let rel_end = vis_end.saturating_sub(pos).min(span_len);

            let chars: Vec<char> = span.content.chars().collect();

            // Before underline.
            if rel_start > 0 {
                let before: String = chars[..rel_start].iter().collect();
                result.push(Span::styled(before, span.style));
            }
            // Underline portion.
            let underlined: String = chars[rel_start..rel_end].iter().collect();
            result.push(Span::styled(
                underlined,
                span.style.fg(accent).add_modifier(Modifier::UNDERLINED),
            ));
            // After underline.
            if rel_end < span_len {
                let after: String = chars[rel_end..].iter().collect();
                result.push(Span::styled(after, span.style));
            }
        }
        pos = span_end;
    }
    result
}

/// Apply Vimium-style hint labels to spans, replacing the first 2 characters of each
/// hinted symbol with the label text in accent color + bold.
pub(super) fn apply_hint_labels(
    spans: Vec<Span<'static>>,
    hints: &[&crate::overlay::SymbolHint],
    input: &str,
    h_scroll: usize,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let mut result = spans;
    // Process hints in reverse order so earlier replacements don't shift positions of later ones.
    let mut sorted: Vec<&&crate::overlay::SymbolHint> = hints.iter().collect();
    sorted.sort_by_key(|h| std::cmp::Reverse(h.start_col));

    for hint in sorted {
        let vis_start = hint.start_col.saturating_sub(h_scroll);
        let label_len = hint.label.chars().count();
        let vis_end = vis_start + label_len;

        // Determine if this hint matches the current input.
        let is_matching = input.is_empty() || hint.label.starts_with(input);
        let label_style = if is_matching {
            Style::default()
                .fg(theme.selected_fg)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.muted).bg(Color::Reset)
        };

        // Replace characters at vis_start..vis_end with the label.
        result = replace_span_range(result, vis_start, vis_end, &hint.label, label_style);
    }
    result
}

/// Replace characters in the range `[start..end)` of the span list with `replacement` text
/// in the given style.
fn replace_span_range(
    spans: Vec<Span<'static>>,
    start: usize,
    end: usize,
    replacement: &str,
    style: Style,
) -> Vec<Span<'static>> {
    let mut result = Vec::with_capacity(spans.len() + 4);
    let mut pos: usize = 0;

    for span in spans {
        let span_len = span.content.chars().count();
        let span_end = pos + span_len;

        if span_end <= start || pos >= end {
            // Entirely outside the replacement range.
            result.push(span);
        } else {
            let chars: Vec<char> = span.content.chars().collect();
            let rel_start = start.saturating_sub(pos);
            let rel_end = end.saturating_sub(pos).min(span_len);

            // Before replacement.
            if rel_start > 0 {
                let before: String = chars[..rel_start].iter().collect();
                result.push(Span::styled(before, span.style));
            }
            // Replacement portion (only emit once, from the first overlapping span).
            if pos <= start {
                result.push(Span::styled(replacement.to_string(), style));
            }
            // After replacement.
            if rel_end < span_len {
                let after: String = chars[rel_end..].iter().collect();
                result.push(Span::styled(after, span.style));
            }
        }
        pos = span_end;
    }
    result
}
