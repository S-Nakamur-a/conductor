//! Viewer panel — file content display with diff highlights and review comments.
//!
//! Shows the content of the selected file in the middle column. Lines that
//! have been modified (according to diff_state) are highlighted inline.
//! Review comments are shown as inline badges.

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::{App, Focus};
use crate::diff_state::{DiffLineTag, InlineSegment};
use crate::review_state::ReviewInputMode;
use crate::review_store::ReviewComment;

/// Annotation for a diff line, carrying the tag and optional inline segments.
struct DiffAnnotation {
    tag: DiffLineTag,
    inline_segments: Vec<InlineSegment>,
}

/// Render the viewer (file content) panel into the given area.
pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let vs = &app.viewer_state;
    let focused = app.focus == Focus::Viewer;
    let border_color = if focused { Color::Yellow } else { Color::DarkGray };

    let title = match &vs.current_file {
        Some(path) => {
            if !vs.search_matches.is_empty() {
                format!(
                    " {} [{}/{}] ",
                    path,
                    vs.search_match_idx + 1,
                    vs.search_matches.len()
                )
            } else if !vs.search_query.is_empty() {
                format!(" {path} [no matches] ")
            } else {
                format!(" {path} ")
            }
        }
        None => " (no file selected) ".to_string(),
    };

    let is_expanded = app.expanded_panel == Some(Focus::Viewer);
    let (expand_label, expand_color) = if is_expanded {
        ("[>=<]", Color::Yellow)
    } else {
        ("[<=>]", Color::DarkGray)
    };

    let block = Block::default()
        .title(title)
        .title_top(Line::from(Span::styled(expand_label, Style::default().fg(expand_color))).alignment(Alignment::Right))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    if vs.file_content.is_empty() {
        let placeholder = Paragraph::new("Select a file to view its contents.")
            .style(Style::default().fg(Color::DarkGray))
            .block(block);
        frame.render_widget(placeholder, area);
        return;
    }

    let inner_height = area.height.saturating_sub(2) as usize;
    let gutter_width = digit_count(vs.file_content.len());

    // Build diff annotations: map line_number -> DiffLineTag for current file.
    let diff_annotations = build_diff_annotations(app);

    // Collect line numbers that have review comments (from in-memory cache).
    let comment_lines: std::collections::HashSet<usize> =
        app.review_state.file_comments.keys().copied().collect();

    let lines: Vec<Line> = vs
        .file_content
        .iter()
        .enumerate()
        .skip(vs.file_scroll)
        .take(inner_height)
        .map(|(line_no, content)| {
            let line_1 = line_no + 1;
            let is_selected = vs.is_line_selected(line_1);

            // Diff gutter marker.
            let annotation = diff_annotations.get(&line_1);
            let diff_tag = annotation.map(|a| a.tag);
            let (gutter_prefix, gutter_bg) = match diff_tag {
                Some(DiffLineTag::Insert) => ("+", Some(app.theme.diff_add_bg)),
                Some(DiffLineTag::Delete) => ("-", None),
                _ => (" ", None),
            };

            // Gutter (line number).
            let num = format!("{gutter_prefix}{line_1:>gutter_width$} \u{2502} ");
            let gutter_style = if is_selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::LightBlue)
                    .add_modifier(Modifier::BOLD)
            } else if diff_tag == Some(DiffLineTag::Insert) {
                Style::default().fg(Color::Green)
            } else if diff_tag == Some(DiffLineTag::Delete) {
                Style::default().fg(Color::Red)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let gutter_span = Span::styled(num, gutter_style);

            // Comment badge.
            let badge = if comment_lines.contains(&line_1) {
                Span::styled("\u{25c6} ", Style::default().fg(Color::Yellow))
            } else {
                Span::raw("  ")
            };

            // Content styling.
            let is_match = vs.search_matches.contains(&line_no);
            let is_current_match =
                vs.search_matches.get(vs.search_match_idx) == Some(&line_no);

            let content_spans: Vec<Span> = if is_current_match {
                vec![Span::styled(
                    content.to_string(),
                    Style::default().fg(Color::Black).bg(Color::Yellow),
                )]
            } else if is_match {
                vec![Span::styled(
                    content.to_string(),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )]
            } else if is_selected {
                vec![Span::styled(
                    content.to_string(),
                    Style::default().bg(Color::DarkGray).fg(Color::White),
                )]
            } else if let Some(ann) = annotation {
                if !ann.inline_segments.is_empty() {
                    // Word-level diff: render each segment with appropriate background.
                    let (diff_bg, emphasis_bg) = match ann.tag {
                        DiffLineTag::Insert => (app.theme.diff_add_bg, app.theme.diff_add_bg_emphasis),
                        DiffLineTag::Delete => (app.theme.diff_del_bg, app.theme.diff_del_bg_emphasis),
                        _ => (Color::Reset, Color::Reset),
                    };

                    if ann.tag == DiffLineTag::Insert {
                        // Insert lines: the viewer shows the new file, so
                        // highlighted_lines[line_no] corresponds to this line.
                        // Try full merge (syntax fg + word-diff emphasis bg).
                        // If that fails, still use syntax highlighting with
                        // line-level diff bg (lose word emphasis, keep colours).
                        vs.highlighted_lines.get(line_no)
                            .filter(|t| !t.is_empty())
                            .and_then(|tokens| merge_syntax_with_inline(
                                &ann.inline_segments, tokens, diff_bg, emphasis_bg,
                            ))
                            .unwrap_or_else(|| syntax_spans_for_line(vs, line_no, Some(diff_bg)))
                    } else {
                        // Delete lines: the deleted content doesn't exist in
                        // the viewer's file, so we can't use syntax tokens.
                        render_inline_diff_spans(
                            &ann.inline_segments,
                            diff_bg,
                            emphasis_bg,
                        )
                    }
                } else {
                    // Line-level diff only: use syntax highlighting with diff bg.
                    let diff_bg = match ann.tag {
                        DiffLineTag::Insert => Some(app.theme.diff_add_bg),
                        DiffLineTag::Delete => Some(app.theme.diff_del_bg),
                        _ => None,
                    };
                    syntax_spans_for_line(vs, line_no, diff_bg)
                }
            } else {
                syntax_spans_for_line(vs, line_no, gutter_bg)
            };

            // Apply horizontal scroll to content spans.
            let content_spans = h_scroll_spans(content_spans, vs.h_scroll);

            let mut spans = vec![gutter_span, badge];
            spans.extend(content_spans);
            Line::from(spans)
        })
        .collect();

    // Clear the area first to avoid stale content when scrolling.
    // ratatui 0.29 does not reset cells between frames, so without this
    // the diff algorithm may leave old characters visible.
    frame.render_widget(ratatui::widgets::Clear, area);

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);

    // Show selection hint overlay.
    if let Some((start, end)) = vs.selected_range() {
        let hint = if start == end {
            format!(" L{start} selected \u{2502} c: comment  Esc: clear ")
        } else {
            format!(" L{start}-L{end} selected \u{2502} c: comment  Esc: clear ")
        };
        let hint_width = hint.len().min(area.width.saturating_sub(2) as usize) as u16;
        let y = area.y + area.height.saturating_sub(2);
        let hint_area = Rect::new(area.x + 1, y, hint_width, 1);
        frame.render_widget(ratatui::widgets::Clear, hint_area);
        let hint_widget = Paragraph::new(Span::styled(
            hint,
            Style::default().fg(Color::Black).bg(Color::LightBlue),
        ));
        frame.render_widget(hint_widget, hint_area);
    }

    // Show comment preview overlay when the cursor line has comments.
    if !vs.search_active && app.review_state.input_mode == ReviewInputMode::Normal {
        let cursor_line = if let Some(line) = vs.comment_preview_line {
            line
        } else if let Some((start, _)) = vs.selected_range() {
            start
        } else {
            vs.file_scroll + 1
        };
        if let Some(comments) = app.review_state.file_comments.get(&cursor_line) {
            let has_selection = vs.selected_range().is_some();
            render_comment_preview(frame, area, comments, cursor_line, has_selection);
        }
    }

    // Show search input overlay.
    if vs.search_active {
        render_search_box(frame, area, &vs.search_query);
    }
}

/// Render a comment preview overlay at the bottom of the viewer area.
fn render_comment_preview(
    frame: &mut Frame,
    area: Rect,
    comments: &[ReviewComment],
    line: usize,
    has_selection: bool,
) {
    let max_comments: usize = 3;
    let content_count = comments.len().min(max_comments);
    let height = (1 + content_count) as u16; // 1 header + N comments
    let width = area.width.saturating_sub(2);

    // Position above the selection hint overlay if a selection is active.
    let offset_for_selection = if has_selection { 1_u16 } else { 0 };
    let y = area
        .y
        .saturating_add(area.height)
        .saturating_sub(height + 1 + offset_for_selection);
    let preview_area = Rect::new(area.x + 1, y, width, height);

    frame.render_widget(ratatui::widgets::Clear, preview_area);

    let mut lines = Vec::new();

    // Header line.
    let count_label = if comments.len() == 1 {
        "1 comment".to_string()
    } else {
        format!("{} comments", comments.len())
    };
    lines.push(Line::from(vec![
        Span::styled(
            format!(" L{line} "),
            Style::default().fg(Color::Black).bg(Color::Yellow),
        ),
        Span::styled(
            format!(" {count_label}"),
            Style::default().fg(Color::DarkGray),
        ),
    ]));

    // Comment bodies (truncated to fit).
    for comment in comments.iter().take(max_comments) {
        let kind_badge = crate::ui::review::kind_badge_span(comment.kind);
        let author_label = match comment.author {
            crate::review_store::Author::User => "you",
            crate::review_store::Author::Claude => "claude",
        };
        let prefix_len = 4 + author_label.len() + 2; // "[S] " + author + ": "
        let max_body = (width as usize).saturating_sub(prefix_len);
        let body: String = comment
            .body
            .replace('\n', " ")
            .chars()
            .take(max_body)
            .collect();

        lines.push(Line::from(vec![
            kind_badge,
            Span::styled(
                format!("{author_label}: "),
                Style::default().fg(Color::Cyan),
            ),
            Span::styled(body, Style::default().fg(Color::White)),
        ]));
    }

    let paragraph = Paragraph::new(lines).style(Style::default().bg(Color::Rgb(30, 30, 50)));
    frame.render_widget(paragraph, preview_area);
}

/// Build a map of line_number -> DiffAnnotation for the currently viewed file.
///
/// Searches both committed and uncommitted file lists. Uncommitted annotations
/// are added first so that committed annotations don't overwrite them (the
/// viewer shows the workdir version of the file, so uncommitted changes are
/// more relevant).
fn build_diff_annotations(app: &App) -> std::collections::HashMap<usize, DiffAnnotation> {
    use crate::diff_state::FileDiff;

    let mut annotations = std::collections::HashMap::new();
    let current_file = match &app.viewer_state.current_file {
        Some(f) => f,
        None => return annotations,
    };

    let insert_annotations = |file_diff: &FileDiff, map: &mut std::collections::HashMap<usize, DiffAnnotation>| {
        for hunk in &file_diff.hunks {
            for line in &hunk.lines {
                match line.tag {
                    DiffLineTag::Insert => {
                        if let Some(n) = line.new_line_no {
                            map.entry(n).or_insert_with(|| DiffAnnotation {
                                tag: DiffLineTag::Insert,
                                inline_segments: line.inline_segments.clone(),
                            });
                        }
                    }
                    DiffLineTag::Delete => {
                        if let Some(n) = line.old_line_no {
                            map.entry(n).or_insert_with(|| DiffAnnotation {
                                tag: DiffLineTag::Delete,
                                inline_segments: line.inline_segments.clone(),
                            });
                        }
                    }
                    DiffLineTag::Equal => {}
                }
            }
        }
    };

    // Uncommitted first (takes priority in the viewer).
    for file_diff in &app.diff_state.uncommitted_files {
        if file_diff.path == *current_file {
            insert_annotations(file_diff, &mut annotations);
            break;
        }
    }

    // Committed second (or_insert prevents overwriting uncommitted).
    for file_diff in &app.diff_state.committed_files {
        if file_diff.path == *current_file {
            insert_annotations(file_diff, &mut annotations);
            break;
        }
    }

    annotations
}

/// Render intra-line diff segments with emphasis highlighting (plain white fg).
/// Used for Delete lines where syntax tokens are unavailable.
fn render_inline_diff_spans(
    segments: &[InlineSegment],
    diff_bg: Color,
    emphasis_bg: Color,
) -> Vec<Span<'static>> {
    segments
        .iter()
        .map(|seg| {
            let bg = if seg.emphasized { emphasis_bg } else { diff_bg };
            Span::styled(
                seg.text.clone(),
                Style::default().fg(Color::White).bg(bg),
            )
        })
        .collect()
}

/// Merge syntax highlighting foreground colours with word-diff background
/// colours. Returns `None` if the expanded segment text does not match the
/// syntax token text (so the caller can fall back to plain rendering).
fn merge_syntax_with_inline(
    segments: &[InlineSegment],
    syntax_tokens: &[(Style, String)],
    diff_bg: Color,
    emphasis_bg: Color,
) -> Option<Vec<Span<'static>>> {
    // Build expanded text and per-byte emphasis flag from inline segments.
    let mut expanded_text = String::new();
    let mut byte_emphasis: Vec<bool> = Vec::new();

    for seg in segments {
        let trimmed = seg.text.trim_end_matches('\n').trim_end_matches('\r');
        let expanded = expand_tabs(trimmed, 4);
        byte_emphasis.resize(byte_emphasis.len() + expanded.len(), seg.emphasized);
        expanded_text.push_str(&expanded);
    }

    // Build per-byte fg style from syntax tokens.
    let mut syntax_text = String::new();
    let mut byte_fg: Vec<Style> = Vec::new();

    for (style, text) in syntax_tokens {
        byte_fg.resize(byte_fg.len() + text.len(), *style);
        syntax_text.push_str(text);
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

        result.push(Span::styled(
            expanded_text[start..i].to_string(),
            fg.bg(bg),
        ));
    }

    Some(result)
}

/// Expand tab characters to spaces, matching the viewer's tab expansion.
fn expand_tabs(line: &str, tab_width: usize) -> String {
    if !line.contains('\t') {
        return line.to_string();
    }
    let mut result = String::with_capacity(line.len());
    let mut col = 0;
    for ch in line.chars() {
        if ch == '\t' {
            let spaces = tab_width - (col % tab_width);
            for _ in 0..spaces {
                result.push(' ');
            }
            col += spaces;
        } else {
            result.push(ch);
            col += 1;
        }
    }
    result
}

fn render_search_box(frame: &mut Frame, area: Rect, query: &str) {
    let height = 1_u16;
    let y = area.y + area.height.saturating_sub(height + 1);
    let search_area = Rect::new(area.x + 1, y, area.width.saturating_sub(2), height);

    frame.render_widget(ratatui::widgets::Clear, search_area);

    let text = format!("/{query}\u{2588}");
    let paragraph = Paragraph::new(Span::styled(
        text,
        Style::default().fg(Color::Yellow),
    ));
    frame.render_widget(paragraph, search_area);
}

// ── Syntax highlighting via cached syntect data ─────────────────────────

/// Return ratatui `Span`s for a single line from the syntect highlight cache.
///
/// If a `diff_bg` is provided, the token foreground colours are preserved but
/// the background is overridden with the diff colour.  When no cache entry
/// exists for the line, a plain white fallback is returned.
fn syntax_spans_for_line(
    vs: &crate::viewer_state::ViewerState,
    line_no: usize,
    diff_bg: Option<Color>,
) -> Vec<Span<'static>> {
    if let Some(tokens) = vs.highlighted_lines.get(line_no) {
        tokens
            .iter()
            .map(|(style, text)| {
                let s = if let Some(bg) = diff_bg {
                    // Keep token fg, override bg with diff colour.
                    style.bg(bg)
                } else {
                    *style
                };
                Span::styled(text.clone(), s)
            })
            .collect()
    } else {
        // Fallback: plain white text.
        let text = vs
            .file_content
            .get(line_no)
            .cloned()
            .unwrap_or_default();
        vec![Span::styled(text, Style::default().fg(Color::White))]
    }
}

/// Skip `offset` characters from the beginning of a sequence of `Span`s,
/// preserving per-span styling.  Returns the remaining spans.
fn h_scroll_spans(spans: Vec<Span<'static>>, offset: usize) -> Vec<Span<'static>> {
    if offset == 0 {
        return spans;
    }
    let mut remaining = offset;
    let mut result: Vec<Span<'static>> = Vec::new();
    for span in spans {
        let char_count = span.content.chars().count();
        if remaining >= char_count {
            remaining -= char_count;
            continue;
        }
        if remaining > 0 {
            let s: String = span.content.chars().skip(remaining).collect();
            result.push(Span::styled(s, span.style));
            remaining = 0;
        } else {
            result.push(span);
        }
    }
    result
}

fn digit_count(n: usize) -> usize {
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
