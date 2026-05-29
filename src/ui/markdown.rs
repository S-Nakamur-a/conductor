//! Minimal markdown renderer for the change-summary view.
//!
//! Renders a change summary written in Markdown into styled, word-wrapped
//! ratatui `Line`s. It is deliberately **not** a CommonMark implementation: the
//! summary is a short, self-authored PR-description-style note, so a small
//! line-oriented parser covers the useful subset (headings, lists, block
//! quotes, fenced code blocks, horizontal rules, and inline `code`/**bold**/
//! *italic*) without pulling in a markdown crate.
//!
//! Design notes:
//! - **Backward compatible.** Plain text containing no Markdown syntax flows
//!   through as ordinary paragraphs — visually identical to the old plain-text
//!   summary, one author line per output paragraph.
//! - **Fenced code blocks reuse syntect** (the same engine the file viewer
//!   uses) via the caller-provided `SyntaxSet`/`Theme`. An unknown or missing
//!   language falls back to plain text — never a panic.
//! - **Total function.** Any input string, for any width (including 0), yields
//!   a `Vec<Line>` without panicking. Each produced line's display width stays
//!   within `width`.
//! - Underscore emphasis (`_x_`) is intentionally **not** supported so that
//!   `snake_case` identifiers in prose are never mangled. Inline emphasis uses
//!   `*`/`**` only and requires non-space flanking, so `2 * 3` stays literal.
//!
//! The single public entry point is [`render_markdown`]. Everything else is a
//! private, individually testable helper.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::Theme as SyntectTheme;
use syntect::parsing::SyntaxSet;
use unicode_width::UnicodeWidthChar;

use crate::theme::Theme;

/// A block of the parsed summary. The parser is line-oriented, so most blocks
/// map to a single source line; only `CodeBlock` spans multiple lines.
#[derive(Debug, PartialEq)]
enum MdBlock {
    /// `# heading` .. `###### heading` (level 1–6).
    Heading { level: u8, text: String },
    /// A normal text line. Author line breaks are preserved (one block each).
    Paragraph(String),
    /// `- item` / `* item` / `+ item` or `1. item` / `1) item`.
    ListItem {
        /// `Some("1")` for an ordered item (keeps the author's number), `None`
        /// for a bullet.
        ordered: Option<String>,
        text: String,
        /// Leading-whitespace columns before the marker (nesting indent).
        indent: usize,
    },
    /// `> quoted text`.
    Quote(String),
    /// A fenced code block. `lang` is the info-string's first token (if any).
    CodeBlock {
        lang: Option<String>,
        lines: Vec<String>,
    },
    /// `---` / `***` / `___` (3+ of the same marker).
    Rule,
    /// A blank source line (preserved as paragraph spacing).
    Blank,
}

/// Render Markdown `text` into word-wrapped, styled lines no wider than `width`.
///
/// `syntax_set`/`syntect_theme` are used to highlight fenced code blocks and are
/// expected to be the application's shared instances (see `App::syntax_set`).
pub fn render_markdown(
    text: &str,
    width: usize,
    theme: &Theme,
    syntax_set: &SyntaxSet,
    syntect_theme: &SyntectTheme,
) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut out: Vec<Line<'static>> = Vec::new();
    for block in parse_blocks(text) {
        out.extend(render_block(&block, width, theme, syntax_set, syntect_theme));
    }
    if out.is_empty() {
        out.push(Line::from(""));
    }
    out
}

// ── Parsing ──────────────────────────────────────────────────────────

/// Split `text` into blocks. Lines are split on `\n`; a trailing `\r` (CRLF
/// input) is stripped so fence detection and code bodies stay clean.
fn parse_blocks(text: &str) -> Vec<MdBlock> {
    let lines: Vec<&str> = text.split('\n').map(strip_cr).collect();
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();

        // Fenced code block — consumes lines until a matching close fence (or EOF).
        if let Some((fence_char, fence_len, info)) = fence_open(trimmed) {
            let lang = info
                .split_whitespace()
                .next()
                .map(str::to_string)
                .filter(|s| !s.is_empty());
            let mut body = Vec::new();
            i += 1;
            while i < lines.len() {
                if is_fence_close(lines[i].trim_start(), fence_char, fence_len) {
                    i += 1;
                    break;
                }
                body.push(lines[i].to_string());
                i += 1;
            }
            blocks.push(MdBlock::CodeBlock { lang, lines: body });
            continue;
        }

        if trimmed.is_empty() {
            blocks.push(MdBlock::Blank);
        } else if is_hr(trimmed) {
            blocks.push(MdBlock::Rule);
        } else if let Some((level, htext)) = parse_heading(trimmed) {
            blocks.push(MdBlock::Heading {
                level,
                text: htext,
            });
        } else if let Some(rest) = trimmed.strip_prefix('>') {
            blocks.push(MdBlock::Quote(
                rest.strip_prefix(' ').unwrap_or(rest).to_string(),
            ));
        } else if let Some(item) = parse_list_item(line) {
            blocks.push(item);
        } else {
            blocks.push(MdBlock::Paragraph(trimmed.to_string()));
        }
        i += 1;
    }
    blocks
}

fn strip_cr(s: &str) -> &str {
    s.strip_suffix('\r').unwrap_or(s)
}

/// If `s` opens a code fence, return `(fence_char, fence_len, info_string)`.
/// A fence is 3+ backticks or 3+ tildes at the start of the (trimmed) line.
fn fence_open(s: &str) -> Option<(char, usize, &str)> {
    let first = s.chars().next()?;
    if first != '`' && first != '~' {
        return None;
    }
    let len = s.chars().take_while(|&c| c == first).count();
    if len < 3 {
        return None;
    }
    // `len` equals the byte offset because both fence chars are ASCII.
    Some((first, len, s[len..].trim()))
}

/// A close fence is 3+ (>= open length) of the same char, then only whitespace.
fn is_fence_close(s: &str, fence_char: char, fence_len: usize) -> bool {
    let len = s.chars().take_while(|&c| c == fence_char).count();
    len >= fence_len && s.chars().skip(len).all(char::is_whitespace)
}

/// `---`, `***`, `___` (>= 3 of one marker, spaces allowed between).
fn is_hr(s: &str) -> bool {
    let marks: Vec<char> = s.chars().filter(|c| !c.is_whitespace()).collect();
    if marks.len() < 3 {
        return false;
    }
    let first = marks[0];
    matches!(first, '-' | '*' | '_') && marks.iter().all(|&c| c == first)
}

/// `# ` .. `###### ` → `(level, heading_text)`. A space after the hashes is
/// required (so `#nofilter`, `C#`, issue refs like `#242` stay paragraphs).
fn parse_heading(s: &str) -> Option<(u8, String)> {
    let hashes = s.chars().take_while(|&c| c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &s[hashes..];
    // Require a separating space, except for an otherwise-empty heading ("# ").
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None;
    }
    Some((hashes as u8, rest.trim_start().to_string()))
}

/// `- `/`* `/`+ ` (bullet) or `N. `/`N) ` (ordered) → a `ListItem`.
fn parse_list_item(line: &str) -> Option<MdBlock> {
    let indent = line.len() - line.trim_start().len();
    let s = line.trim_start();

    if let Some(rest) = s
        .strip_prefix("- ")
        .or_else(|| s.strip_prefix("* "))
        .or_else(|| s.strip_prefix("+ "))
    {
        return Some(MdBlock::ListItem {
            ordered: None,
            text: rest.to_string(),
            indent,
        });
    }

    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    if !digits.is_empty() {
        let after = &s[digits.len()..];
        if let Some(rest) = after.strip_prefix(". ").or_else(|| after.strip_prefix(") ")) {
            return Some(MdBlock::ListItem {
                ordered: Some(digits),
                text: rest.to_string(),
                indent,
            });
        }
    }
    None
}

// ── Rendering ────────────────────────────────────────────────────────

fn render_block(
    block: &MdBlock,
    width: usize,
    theme: &Theme,
    syntax_set: &SyntaxSet,
    syntect_theme: &SyntectTheme,
) -> Vec<Line<'static>> {
    match block {
        MdBlock::Blank => vec![Line::from("")],
        MdBlock::Rule => vec![Line::from(Span::styled(
            "\u{2500}".repeat(width),
            Style::default().fg(theme.muted),
        ))],
        MdBlock::Heading { level, text } => {
            let color = if *level <= 2 { theme.accent } else { theme.info };
            let style = Style::default().fg(color).add_modifier(Modifier::BOLD);
            let cells = spans_to_cells(&inline_spans(text, style, theme));
            wrap_cells(&cells, width, false)
        }
        MdBlock::Paragraph(text) => {
            let cells = spans_to_cells(&inline_spans(text, Style::default().fg(theme.fg), theme));
            wrap_cells(&cells, width, false)
        }
        MdBlock::Quote(text) => {
            let inner = width.saturating_sub(2).max(1);
            let style = Style::default()
                .fg(theme.muted)
                .add_modifier(Modifier::ITALIC);
            let cells = spans_to_cells(&inline_spans(text, style, theme));
            let bar = Span::styled("\u{2502} ".to_string(), Style::default().fg(theme.muted));
            with_prefix(wrap_cells(&cells, inner, false), bar.clone(), bar)
        }
        MdBlock::ListItem {
            ordered,
            text,
            indent,
        } => {
            let indent = (*indent).min(8);
            let marker = match ordered {
                Some(num) => format!("{num}. "),
                None => "\u{2022} ".to_string(),
            };
            let prefix_w = indent + display_width(&marker);
            let inner = width.saturating_sub(prefix_w).max(1);
            let cells = spans_to_cells(&inline_spans(text, Style::default().fg(theme.fg), theme));
            let pad = " ".repeat(indent);
            let first = Span::styled(format!("{pad}{marker}"), Style::default().fg(theme.accent));
            let cont = Span::styled(" ".repeat(prefix_w), Style::default());
            with_prefix(wrap_cells(&cells, inner, false), first, cont)
        }
        MdBlock::CodeBlock { lang, lines } => {
            render_code_block(lang.as_deref(), lines, width, theme, syntax_set, syntect_theme)
        }
    }
}

/// Highlight a fenced code block with syntect and wrap it under a left gutter
/// bar. Code is hard-wrapped (not word-wrapped) so nothing is hidden.
fn render_code_block(
    lang: Option<&str>,
    lines: &[String],
    width: usize,
    theme: &Theme,
    syntax_set: &SyntaxSet,
    syntect_theme: &SyntectTheme,
) -> Vec<Line<'static>> {
    let inner = width.saturating_sub(2).max(1);
    let syntax = lang
        .and_then(|l| syntax_set.find_syntax_by_token(l))
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text());
    let mut highlighter = HighlightLines::new(syntax, syntect_theme);
    let bar = Span::styled("\u{258F} ".to_string(), Style::default().fg(theme.muted));
    let fallback = Style::default().fg(theme.fg);

    let mut out = Vec::new();
    for raw in lines {
        // Expand tabs so display-width math (and thus wrapping) stays correct.
        let expanded = raw.replace('\t', "    ");
        let with_nl = format!("{expanded}\n");
        let spans: Vec<Span<'static>> = match highlighter.highlight_line(&with_nl, syntax_set) {
            Ok(ranges) => ranges
                .into_iter()
                .map(|(style, piece)| {
                    let st = syntect_tui::translate_style(style)
                        .unwrap_or_default()
                        .bg(Color::Reset);
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
        out.extend(with_prefix(wrapped, bar.clone(), bar.clone()));
    }
    out
}

// ── Inline parsing ───────────────────────────────────────────────────

/// Parse inline `code`, `**bold**`, and `*italic*` out of `text`, styling the
/// rest with `base`. Unmatched/space-flanked delimiters stay literal.
fn inline_spans(text: &str, base: Style, theme: &Theme) -> Vec<Span<'static>> {
    let code_style = Style::default().fg(theme.warning);
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let mut i = 0;

    while i < n {
        let c = chars[i];

        if c == '`' {
            // Inline code: match the next backtick; content may be anything.
            if let Some(j) = (i + 1..n).find(|&k| chars[k] == '`')
                && j > i + 1
            {
                flush(&mut buf, &mut spans, base);
                spans.push(Span::styled(collect(&chars, i + 1, j), code_style));
                i = j + 1;
                continue;
            }
        } else if c == '*' {
            if i + 1 < n && chars[i + 1] == '*' {
                // Bold: opener `**` must be followed by non-space.
                if i + 2 < n
                    && !chars[i + 2].is_whitespace()
                    && let Some(j) = find_close_bold(&chars, i + 2)
                {
                    flush(&mut buf, &mut spans, base);
                    spans.push(Span::styled(
                        collect(&chars, i + 2, j),
                        base.add_modifier(Modifier::BOLD),
                    ));
                    i = j + 2;
                    continue;
                }
            } else if i + 1 < n
                && !chars[i + 1].is_whitespace()
                && chars[i + 1] != '*'
                && let Some(j) = find_close_italic(&chars, i + 1)
            {
                // Italic: opener `*` followed by non-space.
                flush(&mut buf, &mut spans, base);
                spans.push(Span::styled(
                    collect(&chars, i + 1, j),
                    base.add_modifier(Modifier::ITALIC),
                ));
                i = j + 1;
                continue;
            }
        }

        buf.push(c);
        i += 1;
    }

    flush(&mut buf, &mut spans, base);
    if spans.is_empty() {
        spans.push(Span::styled(String::new(), base));
    }
    spans
}

fn flush(buf: &mut String, spans: &mut Vec<Span<'static>>, style: Style) {
    if !buf.is_empty() {
        spans.push(Span::styled(std::mem::take(buf), style));
    }
}

fn collect(chars: &[char], start: usize, end: usize) -> String {
    chars[start..end].iter().collect()
}

/// Find the closing `**` at or after `from` (right-flanking: preceded by a
/// non-space, with non-empty content).
fn find_close_bold(chars: &[char], from: usize) -> Option<usize> {
    let n = chars.len();
    let mut k = from;
    while k + 1 < n {
        if chars[k] == '*' && chars[k + 1] == '*' && k > from && !chars[k - 1].is_whitespace() {
            return Some(k);
        }
        k += 1;
    }
    None
}

/// Find the closing `*` at or after `from` (right-flanking: preceded by a
/// non-space).
fn find_close_italic(chars: &[char], from: usize) -> Option<usize> {
    (from..chars.len()).find(|&k| chars[k] == '*' && !chars[k - 1].is_whitespace())
}

// ── Span wrapping ────────────────────────────────────────────────────

/// A single display cell: one char carrying its style. The wrapping helpers
/// work at this granularity so styles survive line breaks.
#[derive(Clone)]
struct Cell {
    ch: char,
    style: Style,
}

fn char_width(c: char) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(0)
}

fn display_width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

fn spans_to_cells(spans: &[Span<'static>]) -> Vec<Cell> {
    spans
        .iter()
        .flat_map(|s| s.content.chars().map(move |ch| Cell { ch, style: s.style }))
        .collect()
}

/// Merge a run of cells back into a `Line`, coalescing adjacent same-style cells
/// into one `Span`.
fn cells_to_line(cells: &[Cell]) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let mut cur: Option<Style> = None;
    for cell in cells {
        match cur {
            Some(s) if s == cell.style => buf.push(cell.ch),
            _ => {
                if let Some(s) = cur {
                    spans.push(Span::styled(std::mem::take(&mut buf), s));
                }
                buf.push(cell.ch);
                cur = Some(cell.style);
            }
        }
    }
    if let Some(s) = cur {
        spans.push(Span::styled(buf, s));
    }
    if spans.is_empty() {
        Line::from("")
    } else {
        Line::from(spans)
    }
}

/// Wrap `cells` to `width` display columns. When `hard` is set (code blocks),
/// breaks fall on any cell boundary; otherwise breaks prefer word boundaries
/// and only overlong single words are hard-split. Display width is measured with
/// `unicode-width`, so full-width (CJK) text wraps correctly.
fn wrap_cells(cells: &[Cell], width: usize, hard: bool) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut cur: Vec<Cell> = Vec::new();
    let mut cur_w = 0usize;

    let mut push_line = |cur: &mut Vec<Cell>, cur_w: &mut usize| {
        lines.push(cells_to_line(cur));
        cur.clear();
        *cur_w = 0;
    };

    if hard {
        for cell in cells {
            let cw = char_width(cell.ch);
            if cur_w + cw > width && !cur.is_empty() {
                push_line(&mut cur, &mut cur_w);
            }
            cur.push(cell.clone());
            cur_w += cw;
        }
        if !cur.is_empty() {
            push_line(&mut cur, &mut cur_w);
        }
        if lines.is_empty() {
            lines.push(Line::from(""));
        }
        return lines;
    }

    let n = cells.len();
    let mut i = 0;
    while i < n {
        if cells[i].ch == ' ' {
            // A space: keep it only if it fits on the current (non-empty) line;
            // otherwise drop it as the wrap point so the next line has no
            // leading space.
            if !cur.is_empty() {
                if cur_w < width {
                    cur.push(cells[i].clone());
                    cur_w += 1;
                } else {
                    push_line(&mut cur, &mut cur_w);
                }
            }
            i += 1;
            continue;
        }

        // Gather the next word (run of non-space cells).
        let start = i;
        let mut word_w = 0;
        while i < n && cells[i].ch != ' ' {
            word_w += char_width(cells[i].ch);
            i += 1;
        }
        let word = &cells[start..i];

        if word_w > width {
            // Word longer than a line: flush, then hard-split it.
            if !cur.is_empty() {
                push_line(&mut cur, &mut cur_w);
            }
            for cell in word {
                let cw = char_width(cell.ch);
                if cur_w + cw > width && !cur.is_empty() {
                    push_line(&mut cur, &mut cur_w);
                }
                cur.push(cell.clone());
                cur_w += cw;
            }
        } else {
            if cur_w + word_w > width && !cur.is_empty() {
                push_line(&mut cur, &mut cur_w);
            }
            for cell in word {
                cur.push(cell.clone());
                cur_w += char_width(cell.ch);
            }
        }
    }
    if !cur.is_empty() {
        push_line(&mut cur, &mut cur_w);
    }
    if lines.is_empty() {
        lines.push(Line::from(""));
    }
    lines
}

/// Prepend a gutter/marker prefix to wrapped lines: `first` on the first line,
/// `cont` (a same-width pad) on continuation lines.
fn with_prefix(
    lines: Vec<Line<'static>>,
    first: Span<'static>,
    cont: Span<'static>,
) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .enumerate()
        .map(|(idx, line)| {
            let mut spans = Vec::with_capacity(line.spans.len() + 1);
            spans.push(if idx == 0 { first.clone() } else { cont.clone() });
            spans.extend(line.spans);
            Line::from(spans)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use syntect::highlighting::ThemeSet;

    fn fixtures() -> (Theme, SyntaxSet, SyntectTheme) {
        let theme = Theme::default();
        let syntax_set = two_face::syntax::extra_newlines();
        let syntect_theme = ThemeSet::load_defaults()
            .themes
            .remove("base16-ocean.dark")
            .unwrap();
        (theme, syntax_set, syntect_theme)
    }

    /// Concatenate all span contents of a line into a single string.
    fn line_text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn render(text: &str, width: usize) -> Vec<Line<'static>> {
        let (theme, ss, st) = fixtures();
        render_markdown(text, width, &theme, &ss, &st)
    }

    // ── Parsing ──

    #[test]
    fn plain_text_is_paragraphs_unchanged() {
        let blocks = parse_blocks("Just a normal sentence.\nSecond line.");
        assert_eq!(
            blocks,
            vec![
                MdBlock::Paragraph("Just a normal sentence.".to_string()),
                MdBlock::Paragraph("Second line.".to_string()),
            ]
        );
    }

    #[test]
    fn hash_without_space_is_not_a_heading() {
        // Issue refs / C# / #nofilter must stay paragraphs.
        assert_eq!(
            parse_blocks("fix issue #242 now"),
            vec![MdBlock::Paragraph("fix issue #242 now".to_string())]
        );
        assert_eq!(
            parse_blocks("#nofilter"),
            vec![MdBlock::Paragraph("#nofilter".to_string())]
        );
        assert_eq!(
            parse_blocks("####### too many"),
            vec![MdBlock::Paragraph("####### too many".to_string())]
        );
    }

    #[test]
    fn headings_parse_with_level() {
        assert_eq!(
            parse_blocks("# Title\n### Sub"),
            vec![
                MdBlock::Heading { level: 1, text: "Title".to_string() },
                MdBlock::Heading { level: 3, text: "Sub".to_string() },
            ]
        );
    }

    #[test]
    fn list_items_bullet_and_ordered() {
        assert_eq!(
            parse_blocks("- a\n* b\n1. c\n2) d"),
            vec![
                MdBlock::ListItem { ordered: None, text: "a".to_string(), indent: 0 },
                MdBlock::ListItem { ordered: None, text: "b".to_string(), indent: 0 },
                MdBlock::ListItem { ordered: Some("1".to_string()), text: "c".to_string(), indent: 0 },
                MdBlock::ListItem { ordered: Some("2".to_string()), text: "d".to_string(), indent: 0 },
            ]
        );
        // No space after the marker → plain paragraph.
        assert_eq!(
            parse_blocks("-5 degrees"),
            vec![MdBlock::Paragraph("-5 degrees".to_string())]
        );
    }

    #[test]
    fn unclosed_fence_consumes_to_eof() {
        let blocks = parse_blocks("```rust\nlet x = 1;\nfn y() {}");
        assert_eq!(
            blocks,
            vec![MdBlock::CodeBlock {
                lang: Some("rust".to_string()),
                lines: vec!["let x = 1;".to_string(), "fn y() {}".to_string()],
            }]
        );
    }

    #[test]
    fn fence_without_lang_and_crlf() {
        let blocks = parse_blocks("```\r\ncode\r\n```\r\n");
        assert_eq!(
            blocks,
            vec![
                MdBlock::CodeBlock { lang: None, lines: vec!["code".to_string()] },
                MdBlock::Blank,
            ]
        );
    }

    #[test]
    fn fence_does_not_interpret_inner_markdown() {
        let blocks = parse_blocks("```\n# not a heading\n- not a list\n```");
        assert_eq!(
            blocks,
            vec![MdBlock::CodeBlock {
                lang: None,
                lines: vec!["# not a heading".to_string(), "- not a list".to_string()],
            }]
        );
    }

    #[test]
    fn horizontal_rule_vs_text() {
        assert_eq!(parse_blocks("---"), vec![MdBlock::Rule]);
        assert_eq!(parse_blocks("***"), vec![MdBlock::Rule]);
        assert_eq!(
            parse_blocks("a - b"),
            vec![MdBlock::Paragraph("a - b".to_string())]
        );
    }

    // ── Inline ──

    #[test]
    fn snake_case_and_bare_star_stay_literal() {
        let (theme, _, _) = fixtures();
        let base = Style::default().fg(theme.fg);
        // Underscores are never emphasis.
        let spans = inline_spans("call set_change_summary here", base, &theme);
        assert_eq!(joined(&spans), "call set_change_summary here");
        assert_eq!(spans.len(), 1, "no styled split for snake_case");
        // `2 * 3`: space-flanked star is literal.
        let spans = inline_spans("rate is 2 * 3", base, &theme);
        assert_eq!(joined(&spans), "rate is 2 * 3");
        assert_eq!(spans.len(), 1);
    }

    #[test]
    fn bold_italic_code_are_styled() {
        let (theme, _, _) = fixtures();
        let base = Style::default().fg(theme.fg);

        let spans = inline_spans("a **b** c", base, &theme);
        assert_eq!(joined(&spans), "a b c");
        assert!(
            spans
                .iter()
                .any(|s| s.content == "b" && s.style.add_modifier.contains(Modifier::BOLD))
        );

        let spans = inline_spans("a *b* c", base, &theme);
        assert!(
            spans
                .iter()
                .any(|s| s.content == "b" && s.style.add_modifier.contains(Modifier::ITALIC))
        );

        let spans = inline_spans("use `git` now", base, &theme);
        assert!(spans.iter().any(|s| s.content == "git"));
        assert_eq!(joined(&spans), "use git now");
    }

    #[test]
    fn unclosed_inline_delimiters_stay_literal() {
        let (theme, _, _) = fixtures();
        let base = Style::default().fg(theme.fg);
        for input in ["a `b", "a *b", "a **b", "*", "`"] {
            let spans = inline_spans(input, base, &theme);
            assert_eq!(joined(&spans), input, "input {input:?} should be literal");
        }
    }

    fn joined(spans: &[Span]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    // ── Wrapping / width ──

    #[test]
    fn wraps_to_width_and_preserves_words() {
        let lines = render("the quick brown fox", 9);
        for line in &lines {
            assert!(display_width(&line_text(line)) <= 9);
        }
        assert_eq!(lines.iter().map(line_text).collect::<Vec<_>>().join(" ").replace("  ", " ").trim(), "the quick brown fox");
    }

    #[test]
    fn full_width_cjk_wraps_by_display_width() {
        // 6 full-width chars = 12 columns; at width 10 it must split.
        let lines = render("ああああああ", 10);
        for line in &lines {
            assert!(display_width(&line_text(line)) <= 10);
        }
        let joined: String = lines.iter().map(line_text).collect();
        assert_eq!(joined, "ああああああ");
    }

    #[test]
    fn overlong_token_is_hard_split() {
        let lines = render(&"a".repeat(12), 5);
        let texts: Vec<String> = lines.iter().map(line_text).collect();
        assert_eq!(texts, vec!["aaaaa", "aaaaa", "aa"]);
    }

    #[test]
    fn blank_line_preserved_as_spacing() {
        let lines = render("a\n\nb", 20);
        let texts: Vec<String> = lines.iter().map(line_text).collect();
        assert_eq!(texts, vec!["a", "", "b"]);
    }

    // ── Robustness ──

    #[test]
    fn never_panics_on_adversarial_input() {
        let inputs = [
            "",
            "```",
            "```rust",
            "~~~~",
            "###### ",
            "#",
            ">",
            "- ",
            "**",
            "`",
            "🧑‍🤝‍🧑 *x* `y`",
            "\t\tcode",
            "a\r\nb\r\n```\r\nc",
        ];
        for input in inputs {
            for width in [0usize, 1, 2, 3, 8, 80, 1000] {
                let _ = render(input, width);
            }
        }
    }

    #[test]
    fn unknown_language_falls_back_without_panic() {
        let lines = render("```brainfuck\n+++.\n```", 40);
        let joined: String = lines.iter().map(line_text).collect();
        assert!(joined.contains("+++."));
    }

    #[test]
    fn code_block_is_highlighted_and_gutter_prefixed() {
        let lines = render("```rust\nlet x = 1;\n```", 40);
        assert!(!lines.is_empty());
        // Gutter bar prefixes each code line.
        assert!(line_text(&lines[0]).starts_with('\u{258F}'));
        // syntect splits the line into multiple styled spans.
        assert!(lines[0].spans.len() > 2);
    }
}
