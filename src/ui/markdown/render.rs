//! Block-to-`Line` rendering: turns a single [`MdBlock`] into styled,
//! word-wrapped `Line`s, including fenced-code-block syntax highlighting.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::Theme as SyntectTheme;
use syntect::parsing::SyntaxSet;

use crate::theme::Theme;

use super::MarkdownFlavor;
use super::code_colors::render_code_block_transcript;
use super::inline::inline_spans;
use super::parse::MdBlock;
use super::table::render_table;
use super::table_boxed::render_table_boxed;
use super::wrap::{display_width, spans_to_cells, with_prefix, wrap_cells};

pub(crate) fn render_block(
    block: &MdBlock,
    width: usize,
    theme: &Theme,
    syntax_set: &SyntaxSet,
    syntect_theme: &SyntectTheme,
    flavor: MarkdownFlavor,
) -> Vec<Line<'static>> {
    match block {
        MdBlock::Blank => vec![Line::from("")],
        MdBlock::Rule => vec![Line::from(Span::styled(
            "\u{2500}".repeat(width),
            Style::default().fg(theme.muted),
        ))],
        // Transcript flavor: no colour bar, no underline rule — just bold
        // body-colour text (H1 additionally gets italic + underline, matching
        // native Claude Code; H2+ stay bold-only). The surrounding blank
        // lines are added by `render_markdown_flavored`.
        MdBlock::Heading { level, text } if flavor == MarkdownFlavor::Transcript => {
            let mut modifier = Modifier::BOLD;
            if *level == 1 {
                modifier |= Modifier::ITALIC | Modifier::UNDERLINED;
            }
            let style = Style::default().fg(theme.fg).add_modifier(modifier);
            let cells = spans_to_cells(&inline_spans(text, style, theme, flavor));
            wrap_cells(&cells, width, false)
        }
        MdBlock::Heading { level, text } => {
            // Distinct colour per depth so the heading level reads at a glance
            // (not just "bold text"): H1 accent, H2 info, H3 success, then warm
            // and muted for the rarely-used deep levels.
            let color = match level {
                1 => theme.accent,
                2 => theme.info,
                3 => theme.success,
                4 => theme.warning,
                _ => theme.hint,
            };
            let style = Style::default().fg(color).add_modifier(Modifier::BOLD);
            // A thin left colour bar (heavy box-drawing vertical) anchors the
            // heading to its colour and makes sections pop out of the body text
            // without the heaviness of a solid block.
            let bar = Span::styled(
                "\u{2503} ".to_string(),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            );
            let cont = Span::styled("  ".to_string(), Style::default());
            let inner = width.saturating_sub(2).max(1);
            let cells = spans_to_cells(&inline_spans(text, style, theme, MarkdownFlavor::Rich));
            let mut out = with_prefix(wrap_cells(&cells, inner, false), bar, cont);
            // GitHub draws a bottom border under H1/H2; mirror that with a
            // full-width rule tinted toward the heading colour so the section
            // reads as one coloured block.
            if *level <= 2 {
                out.push(Line::from(Span::styled(
                    "\u{2500}".repeat(width),
                    Style::default().fg(Theme::darken(color, 0.55)),
                )));
            }
            out
        }
        MdBlock::Paragraph(text) => {
            let cells =
                spans_to_cells(&inline_spans(text, Style::default().fg(theme.fg), theme, flavor));
            wrap_cells(&cells, width, false)
        }
        // Native Claude Code marks a quote with a dim `▎` and renders the body
        // in the terminal's default colour, italic (no muted grey — that's
        // Rich-only chrome). Kept as its own arm rather than folding into the
        // Rich arm below: the glyph, its style, and the body colour all differ.
        MdBlock::Quote(text) if flavor == MarkdownFlavor::Transcript => {
            let inner = width.saturating_sub(2).max(1);
            let style = Style::default().fg(theme.fg).add_modifier(Modifier::ITALIC);
            let cells = spans_to_cells(&inline_spans(text, style, theme, flavor));
            let bar = Span::styled("\u{258e} ".to_string(), Style::default().add_modifier(Modifier::DIM));
            with_prefix(wrap_cells(&cells, inner, false), bar.clone(), bar)
        }
        MdBlock::Quote(text) => {
            let inner = width.saturating_sub(2).max(1);
            let style = Style::default()
                .fg(theme.muted)
                .add_modifier(Modifier::ITALIC);
            let cells = spans_to_cells(&inline_spans(text, style, theme, MarkdownFlavor::Rich));
            let bar = Span::styled("\u{2502} ".to_string(), Style::default().fg(theme.muted));
            with_prefix(wrap_cells(&cells, inner, false), bar.clone(), bar)
        }
        MdBlock::ListItem {
            ordered,
            checked,
            text,
            indent,
        } => {
            let indent = (*indent).min(8);
            // Bullet glyph: `•` in the rich UI, `-` in the Claude transcript
            // (both one display column, so the marker-width math is unaffected).
            let bullet = match flavor {
                MarkdownFlavor::Rich => "\u{2022} ",
                MarkdownFlavor::Transcript => "- ",
            };
            // Native Claude Code doesn't special-case GFM task-list syntax: a
            // `- [ ] text`/`- [x] text` source line prints as an ordinary
            // bullet item with the checkbox left in the text, unstyled. Fold
            // the marker back into the body and drop `checked` so the rest of
            // this arm's Rich-only styling below doesn't apply to it.
            let (checked, text): (Option<bool>, String) = if flavor == MarkdownFlavor::Transcript {
                let literal = |mark: &str| {
                    if text.is_empty() {
                        mark.to_string()
                    } else {
                        format!("{mark} {text}")
                    }
                };
                match checked {
                    Some(true) => (None, literal("[x]")),
                    Some(false) => (None, literal("[ ]")),
                    None => (None, text.clone()),
                }
            } else {
                (*checked, text.clone())
            };
            // Marker is a truth table over (checked, ordered).
            let marker = match (checked, ordered) {
                (Some(true), _) => "[x] ".to_string(),
                (Some(false), _) => "[ ] ".to_string(),
                (None, Some(num)) => format!("{num}. "),
                (None, None) => bullet.to_string(),
            };
            // Rich accents bullets/numbers; the transcript keeps them body-colour
            // (like the real Claude Code CLI). A completed task always uses success.
            let marker_color = match (checked, flavor) {
                (Some(true), _) => theme.success,
                (_, MarkdownFlavor::Transcript) => theme.fg,
                (_, MarkdownFlavor::Rich) => theme.accent,
            };
            // Completed items recede so the eye lands on what's left.
            let text_color = if checked == Some(true) {
                theme.muted
            } else {
                theme.fg
            };
            let prefix_w = indent + display_width(&marker);
            let inner = width.saturating_sub(prefix_w).max(1);
            let cells = spans_to_cells(&inline_spans(
                &text,
                Style::default().fg(text_color),
                theme,
                flavor,
            ));
            let pad = " ".repeat(indent);
            let first = Span::styled(format!("{pad}{marker}"), Style::default().fg(marker_color));
            let cont = Span::styled(" ".repeat(prefix_w), Style::default());
            with_prefix(wrap_cells(&cells, inner, false), first, cont)
        }
        MdBlock::CodeBlock { lang, lines } => match flavor {
            MarkdownFlavor::Rich => {
                render_code_block(lang.as_deref(), lines, width, theme, syntax_set, syntect_theme)
            }
            MarkdownFlavor::Transcript => {
                render_code_block_transcript(lang.as_deref(), lines, width, syntax_set)
            }
        },
        MdBlock::Table {
            headers,
            aligns,
            rows,
        } => match flavor {
            MarkdownFlavor::Rich => render_table(headers, aligns, rows, width, theme),
            MarkdownFlavor::Transcript => render_table_boxed(headers, rows, width, theme),
        },
    }
}

/// Highlight a fenced code block with syntect and lay it out as a shaded
/// "card" — every row filled to the full width with `theme.code_bg` and inset
/// by one column on each side, the way GitHub frames a code block. Code is
/// hard-wrapped (not word-wrapped) so nothing is hidden, and a blank padded row
/// above and below gives the card vertical breathing room.
fn render_code_block(
    lang: Option<&str>,
    lines: &[String],
    width: usize,
    theme: &Theme,
    syntax_set: &SyntaxSet,
    syntect_theme: &SyntectTheme,
) -> Vec<Line<'static>> {
    // One column of inset on each side of the code; content wraps in between.
    let inner = width.saturating_sub(2).max(1);
    let code_bg = theme.code_bg;
    let syntax = lang
        .and_then(|l| syntax_set.find_syntax_by_token(l))
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text());
    let mut highlighter = HighlightLines::new(syntax, syntect_theme);
    let fallback = Style::default().fg(theme.fg).bg(code_bg);

    // A full-width blank row in the card colour (top/bottom padding).
    let pad_row = || Line::from(Span::styled(" ".repeat(width), Style::default().bg(code_bg)));

    let mut out = vec![pad_row()];
    for raw in lines {
        // Expand tabs so display-width math (and thus wrapping) stays correct.
        let expanded = raw.replace('\t', "    ");
        let with_nl = format!("{expanded}\n");
        let spans: Vec<Span<'static>> = match highlighter.highlight_line(&with_nl, syntax_set) {
            Ok(ranges) => ranges
                .into_iter()
                .map(|(style, piece)| {
                    // Force the card background under every token so the whole
                    // block reads as one surface regardless of syntect's theme.
                    let st = syntect_tui::translate_style(style)
                        .unwrap_or_default()
                        .bg(code_bg);
                    Span::styled(piece.trim_end_matches('\n').to_string(), st)
                })
                .filter(|s| !s.content.is_empty())
                .collect(),
            Err(_) => vec![Span::styled(expanded.clone(), fallback)],
        };
        let cells = spans_to_cells(&spans);
        let wrapped = if cells.is_empty() {
            vec![Line::from("")]
        } else {
            wrap_cells(&cells, inner, true)
        };
        // Frame each wrapped row: left inset, content, right pad — all in the
        // card colour so the background fills edge to edge.
        for line in wrapped {
            out.push(frame_code_row(line, width, code_bg));
        }
    }
    out.push(pad_row());
    out
}

/// Wrap one already-wrapped code row in the card: a leading inset space, the
/// row's spans, then trailing padding — every cell carrying `code_bg` so the
/// row fills `width` columns of solid card colour.
fn frame_code_row(line: Line<'static>, width: usize, code_bg: Color) -> Line<'static> {
    let inset = Style::default().bg(code_bg);
    let mut spans: Vec<Span<'static>> = Vec::with_capacity(line.spans.len() + 2);
    spans.push(Span::styled(" ".to_string(), inset));
    let mut used = 1usize;
    for span in line.spans {
        used += display_width(&span.content);
        spans.push(span);
    }
    if used < width {
        spans.push(Span::styled(" ".repeat(width - used), inset));
    }
    Line::from(spans)
}
