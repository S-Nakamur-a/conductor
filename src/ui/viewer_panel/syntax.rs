//! Syntax highlighting and diff-annotation helpers, backed by the cached
//! syntect data on `ViewerState`: the diff-annotation cache builder, the
//! word-diff span renderers, and syntax-token-to-`Span` conversion.

use crate::app::App;
use crate::diff_state::{DiffLineTag, InlineSegment};
use ratatui::style::{Color, Style};
use ratatui::text::Span;

/// Ensure the diff annotations cache in `ViewerState` is populated for the
/// currently viewed file. Only rebuilds if the file changed or the cache was
/// invalidated (e.g. after `load_diff()`).
pub(super) fn ensure_diff_annotations_cached(app: &mut App) {
    use crate::diff_state::FileDiff;

    let current_file = app.viewer_state.content.current_file.clone();

    // Check if cache is still valid.
    if app.viewer_state.content.cached_diff_annotations.is_some()
        && app.viewer_state.content.cached_diff_annotations_file == current_file
    {
        return;
    }

    let mut annotations = std::collections::HashMap::new();

    if let Some(ref current) = current_file {
        let insert_annotations = |file_diff: &FileDiff,
                                  map: &mut std::collections::HashMap<
            usize,
            (DiffLineTag, Vec<InlineSegment>),
        >| {
            for hunk in &file_diff.hunks {
                for line in &hunk.lines {
                    if line.tag == DiffLineTag::Insert
                        && let Some(n) = line.new_line_no
                    {
                        map.entry(n)
                            .or_insert_with(|| (DiffLineTag::Insert, line.inline_segments.clone()));
                    }
                }
            }
        };

        // Uncommitted first (takes priority in the viewer).
        for file_diff in &app.diff_state.uncommitted_files {
            if file_diff.path == *current {
                insert_annotations(file_diff, &mut annotations);
                break;
            }
        }

        // Committed second (or_insert prevents overwriting uncommitted).
        for file_diff in &app.diff_state.committed_files {
            if file_diff.path == *current {
                insert_annotations(file_diff, &mut annotations);
                break;
            }
        }
    }

    app.viewer_state.content.cached_diff_annotations = Some(annotations);
    app.viewer_state.content.cached_diff_annotations_file = current_file;
}

/// Render intra-line diff segments with emphasis highlighting.
/// Used for Delete lines where syntax tokens are unavailable; `fg` is the
/// plain text color (the active theme's foreground).
pub(super) fn render_inline_diff_spans(
    segments: &[InlineSegment],
    diff_bg: Color,
    emphasis_bg: Color,
    fg: Color,
    tab_width: usize,
) -> Vec<Span<'static>> {
    segments
        .iter()
        .map(|seg| {
            let bg = if seg.emphasized { emphasis_bg } else { diff_bg };
            let text = expand_tabs(
                seg.text.trim_end_matches('\n').trim_end_matches('\r'),
                tab_width,
            );
            Span::styled(text, Style::default().fg(fg).bg(bg))
        })
        .collect()
}

/// Merge syntax highlighting foreground colours with word-diff background
/// colours. Returns `None` if the expanded segment text does not match the
/// syntax token text (so the caller can fall back to plain rendering).
pub(super) fn merge_syntax_with_inline(
    segments: &[InlineSegment],
    syntax_tokens: &[(Style, String)],
    diff_bg: Color,
    emphasis_bg: Color,
    tab_width: usize,
    party: Option<f64>,
) -> Option<Vec<Span<'static>>> {
    // Build expanded text and per-byte emphasis flag from inline segments.
    // Tabs are expanded with a *shared* column counter across segments so the
    // result matches the column-correct expansion of the syntax tokens below.
    let mut expanded_text = String::new();
    let mut byte_emphasis: Vec<bool> = Vec::new();

    let mut col = 0;
    for seg in segments {
        let trimmed = seg.text.trim_end_matches('\n').trim_end_matches('\r');
        let expanded = expand_tabs_at(trimmed, tab_width, &mut col);
        byte_emphasis.resize(byte_emphasis.len() + expanded.len(), seg.emphasized);
        expanded_text.push_str(&expanded);
    }

    // Build per-byte fg style from syntax tokens. The syntax cache stores raw
    // (un-expanded) tabs, so expand them here too — using the same shared
    // column counter — otherwise any line containing a tab would fail the
    // equality check below and silently lose its syntax + emphasis colouring.
    let mut syntax_text = String::new();
    let mut byte_fg: Vec<Style> = Vec::new();

    let mut col = 0;
    for (style, text) in syntax_tokens {
        let trimmed = text.trim_end_matches('\n').trim_end_matches('\r');
        let expanded = expand_tabs_at(trimmed, tab_width, &mut col);
        byte_fg.resize(byte_fg.len() + expanded.len(), *style);
        syntax_text.push_str(&expanded);
    }

    // The texts must match after tab expansion; bail out otherwise.
    if expanded_text != syntax_text {
        return None;
    }

    let len = expanded_text.len();
    let mut result: Vec<Span<'static>> = Vec::new();
    let mut i = 0;

    while i < len {
        let start = i;
        let emph = byte_emphasis[i];
        let fg = byte_fg[i];
        let bg = if emph { emphasis_bg } else { diff_bg };

        i += 1;
        while i < len {
            let next_emph = byte_emphasis[i];
            let next_fg_color = byte_fg[i].fg;
            if next_emph != emph || next_fg_color != fg.fg {
                break;
            }
            i += 1;
        }

        // Ensure we land on a UTF-8 char boundary.
        while i < len && !expanded_text.is_char_boundary(i) {
            i += 1;
        }

        result.push(Span::styled(expanded_text[start..i].to_string(), fg.bg(bg)));
    }

    // Party mode: recolour the merged tokens with a flowing rainbow while
    // keeping their diff backgrounds intact.
    if let Some(phase) = party {
        for (idx, span) in result.iter_mut().enumerate() {
            span.style.fg = Some(crate::ui::party::rainbow(phase + idx as f64 * 23.0));
        }
    }

    Some(result)
}

/// Expand tab characters to spaces, matching the viewer's tab expansion.
fn expand_tabs(line: &str, tab_width: usize) -> String {
    if !line.contains('\t') {
        return line.to_string();
    }
    let mut col = 0;
    expand_tabs_at(line, tab_width, &mut col)
}

/// Expand tabs starting from column `col`, advancing `col` past the piece.
///
/// Threading a shared `col` across consecutive pieces of one line keeps tab
/// stops column-correct, so two different tokenisations of the same line
/// (word-diff segments vs. syntax tokens) expand to identical text.
fn expand_tabs_at(piece: &str, tab_width: usize, col: &mut usize) -> String {
    let mut result = String::with_capacity(piece.len());
    for ch in piece.chars() {
        if ch == '\t' {
            let spaces = tab_width - (*col % tab_width);
            for _ in 0..spaces {
                result.push(' ');
            }
            *col += spaces;
        } else {
            result.push(ch);
            *col += 1;
        }
    }
    result
}

/// Return ratatui `Span`s for a single line from the syntect highlight cache.
///
/// If a `diff_bg` is provided, the token foreground colours are preserved but
/// the background is overridden with the diff colour.  When no cache entry
/// exists for the line, a plain white fallback is returned.
pub(super) fn syntax_spans_for_line(
    vs: &crate::viewer::ViewerState,
    line_no: usize,
    diff_bg: Option<Color>,
    fg: Color,
    party: Option<f64>,
) -> Vec<Span<'static>> {
    if let Some(tokens) = vs.content.highlighted_lines.get(line_no) {
        tokens
            .iter()
            .enumerate()
            .map(|(idx, (style, text))| {
                let mut s = if let Some(bg) = diff_bg {
                    // Keep token fg, override bg with diff colour.
                    style.bg(bg)
                } else {
                    *style
                };
                // Party mode: recolour each token (boundaries preserved) with a
                // flowing rainbow so the whole line goes flashy.
                if let Some(phase) = party {
                    s.fg = Some(crate::ui::party::rainbow(
                        phase + line_no as f64 * 7.0 + idx as f64 * 23.0,
                    ));
                }
                Span::styled(text.clone(), s)
            })
            .collect()
    } else {
        // Fallback: plain text in the theme foreground.
        let text = vs
            .content
            .file_content
            .get(line_no)
            .cloned()
            .unwrap_or_default();
        let color = match party {
            Some(phase) => crate::ui::party::rainbow(phase + line_no as f64 * 7.0),
            None => fg,
        };
        vec![Span::styled(text, Style::default().fg(color))]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(text: &str, emphasized: bool) -> InlineSegment {
        InlineSegment {
            text: text.to_string(),
            emphasized,
        }
    }

    #[test]
    fn merge_handles_tabbed_lines() {
        // A line "\tlet x" highlighted as two syntax tokens carrying a raw tab.
        // The word-diff segments expand the tab; the syntax tokens must be
        // expanded the same way or the merge silently drops to plain rendering.
        let segments = vec![seg("\tlet ", false), seg("x", true)];
        let syntax_tokens = vec![
            (Style::default().fg(Color::Red), "\t".to_string()),
            (Style::default().fg(Color::Blue), "let x".to_string()),
        ];
        let merged = merge_syntax_with_inline(
            &segments,
            &syntax_tokens,
            Color::Rgb(0, 40, 0),
            Color::Rgb(0, 80, 0),
            4,
            None,
        );
        // Before the tab fix this returned None (texts mismatched on the tab).
        let spans = merged.expect("tabbed line should merge, not fall back to plain");
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, "    let x"); // tab expanded to 4 spaces at column 0
    }

    #[test]
    fn merge_bails_on_text_mismatch() {
        // Genuinely different text (not just tabs) must still bail out so the
        // caller can fall back to plain rendering.
        let segments = vec![seg("foo", false)];
        let syntax_tokens = vec![(Style::default(), "bar".to_string())];
        let merged = merge_syntax_with_inline(
            &segments,
            &syntax_tokens,
            Color::Rgb(0, 40, 0),
            Color::Rgb(0, 80, 0),
            4,
            None,
        );
        assert!(merged.is_none());
    }
}
