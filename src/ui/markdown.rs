//! Minimal markdown renderer for the change-summary view.
//!
//! Renders a change summary written in Markdown into styled, word-wrapped
//! ratatui `Line`s. It is deliberately **not** a CommonMark implementation: the
//! summary is a short, self-authored PR-description-style note, so a small
//! line-oriented parser covers the useful subset (headings, lists, task-list
//! checkboxes, block quotes, fenced code blocks, horizontal rules, GFM tables,
//! links, and inline `code`/**bold**/*italic*/~~strikethrough~~) without
//! pulling in a markdown crate.
//!
//! - **Links `[text](url)`** render the text as an underlined `info`-coloured
//!   run followed by the URL in a recessive, muted parenthetical. Terminals
//!   can't reliably click links, so keeping the URL visible lets the reader
//!   copy it; a self-titled or empty link collapses to just the URL.
//! - **Task checkboxes `- [ ]`/`- [x]`** use ASCII brackets (not `☐`/`☑`,
//!   whose East-Asian *ambiguous* width misaligns in CJK-wide terminals); the
//!   `[x]` is coloured and completed items' text is muted so the eye lands on
//!   what's left.
//! - **Strikethrough `~~x~~`** applies `CROSSED_OUT` *and* a muted colour, so
//!   the "removed/deprecated" meaning survives even where the terminal ignores
//!   the SGR 9 escape.
//! - **Tables** render borderless (bold header, a rule, aligned rows) rather
//!   than with box-drawing — borders are too width-hungry for the narrow
//!   summary column. Over-wide cells truncate with `…`. A future refinement
//!   could fall back to a `key: value` list when even truncation can't fit.
//! - **Headings** colour and bold their text; H1/H2 also get a full-width
//!   underline rule, echoing GitHub's bottom border on top-level sections.
//! - **Code — fenced and inline `code`** sits on a shaded `code_bg` "card"
//!   (the background carries the signal, not a lone accent colour). A fenced
//!   block fills every row edge-to-edge with that card colour, padded above and
//!   below, the way GitHub frames a code block. Callers rendering onto a tinted
//!   surface (the comment thread box) use [`apply_background`] to fill the
//!   non-code gaps so the whole block shares one background.
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
        /// GFM task marker: `None` = plain item, `Some(false)` = `[ ]` (open),
        /// `Some(true)` = `[x]` (done).
        checked: Option<bool>,
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
    /// A GFM pipe table: a header row, an alignment row, and zero or more body
    /// rows. `aligns` carries one entry per header column.
    Table {
        headers: Vec<String>,
        aligns: Vec<Align>,
        rows: Vec<Vec<String>>,
    },
    /// `---` / `***` / `___` (3+ of the same marker).
    Rule,
    /// A blank source line (preserved as paragraph spacing).
    Blank,
}

/// Per-column text alignment for a [`MdBlock::Table`], from the delimiter row's
/// colons (`:--` left, `--:` right, `:-:` center).
#[derive(Debug, PartialEq, Clone, Copy)]
enum Align {
    Left,
    Center,
    Right,
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
    // Track whether the previous block was blank so headings get one (and only
    // one) blank line of breathing room above them — GitHub-style section
    // separation that makes the structure scannable at a glance.
    let mut prev_blank = true;
    for block in parse_blocks(text) {
        if matches!(block, MdBlock::Heading { .. }) && !prev_blank {
            out.push(Line::from(""));
        }
        prev_blank = matches!(block, MdBlock::Blank);
        out.extend(render_block(&block, width, theme, syntax_set, syntect_theme));
    }
    if out.is_empty() {
        out.push(Line::from(""));
    }
    out
}

/// Caches [`render_markdown`] output per stable id, so comment/reply bodies in
/// the inline thread box aren't re-parsed/highlighted every frame (the diff is
/// re-rendered at 60fps). Stores the **background-agnostic** lines — callers
/// apply [`apply_background`] afterwards (cheap) — and invalidates an entry when
/// its body or wrap width changes, or the whole cache when the theme changes.
#[derive(Default)]
pub struct MarkdownCache {
    entries: std::cell::RefCell<std::collections::HashMap<String, CacheEntry>>,
    theme_fp: std::cell::Cell<u64>,
}

struct CacheEntry {
    body: String,
    width: usize,
    lines: Vec<Line<'static>>,
}

impl MarkdownCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Cached lines for `key` when body/width/theme are unchanged, else render
    /// and store. Returned lines carry no explicit background.
    pub fn render(
        &self,
        key: &str,
        body: &str,
        width: usize,
        theme: &Theme,
        syntax_set: &SyntaxSet,
        syntect_theme: &SyntectTheme,
    ) -> Vec<Line<'static>> {
        // A theme switch changes colours baked into the cached spans, so drop
        // every entry when the theme fingerprint moves.
        let fp = theme_fingerprint(theme);
        if self.theme_fp.get() != fp {
            self.entries.borrow_mut().clear();
            self.theme_fp.set(fp);
        }
        if let Some(e) = self.entries.borrow().get(key)
            && e.body == body
            && e.width == width
        {
            return e.lines.clone();
        }
        let lines = render_markdown(body, width, theme, syntax_set, syntect_theme);
        self.entries.borrow_mut().insert(
            key.to_string(),
            CacheEntry {
                body: body.to_string(),
                width,
                lines: lines.clone(),
            },
        );
        lines
    }
}

/// Fold the theme colours that affect Markdown rendering into one number, so a
/// theme change is detectable without storing the whole theme per entry.
fn theme_fingerprint(theme: &Theme) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for c in [
        theme.fg,
        theme.accent,
        theme.info,
        theme.muted,
        theme.success,
        theme.warning,
        theme.hint,
        theme.border_secondary,
        theme.code_bg,
        theme.code_fg,
    ] {
        let bits = match c {
            Color::Rgb(r, g, b) => ((r as u32) << 16) | ((g as u32) << 8) | b as u32,
            _ => u32::MAX,
        };
        bits.hash(&mut h);
    }
    h.finish()
}

/// Paint `bg` behind every span that doesn't already carry its own background.
///
/// [`render_markdown`] leaves ordinary text with no background (so it sits on
/// whatever surface is drawn behind it) but gives code its own `code_bg` card.
/// Callers that render markdown onto a tinted surface — e.g. the comment thread
/// box's `comment_preview_bg` — use this to fill the gaps so the whole block
/// shares one background, while code cards keep their distinct shade.
pub fn apply_background(lines: &mut [Line<'static>], bg: Color) {
    for line in lines {
        for span in &mut line.spans {
            if span.style.bg.is_none() {
                span.style = span.style.bg(bg);
            }
        }
    }
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

        // GFM table — a `|`-bearing line immediately followed by a valid
        // delimiter row. The lookahead helper consumes the whole table (and
        // returns `None`, eating nothing, when it isn't really a table).
        if let Some((table, consumed)) = parse_table_at(&lines, i) {
            blocks.push(table);
            i += consumed;
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

/// `- `/`* `/`+ ` (bullet) or `N. `/`N) ` (ordered) → a `ListItem`. A leading
/// GFM task marker (`[ ] `/`[x] `) on the item text is split off into `checked`.
fn parse_list_item(line: &str) -> Option<MdBlock> {
    let indent = line.len() - line.trim_start().len();
    let s = line.trim_start();

    if let Some(rest) = s
        .strip_prefix("- ")
        .or_else(|| s.strip_prefix("* "))
        .or_else(|| s.strip_prefix("+ "))
    {
        let (checked, text) = split_task_marker(rest);
        return Some(MdBlock::ListItem {
            ordered: None,
            checked,
            text: text.to_string(),
            indent,
        });
    }

    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    if !digits.is_empty() {
        let after = &s[digits.len()..];
        if let Some(rest) = after.strip_prefix(". ").or_else(|| after.strip_prefix(") ")) {
            let (checked, text) = split_task_marker(rest);
            return Some(MdBlock::ListItem {
                ordered: Some(digits),
                checked,
                text: text.to_string(),
                indent,
            });
        }
    }
    None
}

/// Split a leading GFM task marker off list-item text. `"[ ] foo"` →
/// `(Some(false), "foo")`; `"[x] foo"`/`"[X] foo"` → `(Some(true), "foo")`; an
/// empty task `"[ ]"` → `(Some(_), "")`. A marker must be followed by a space or
/// end-of-string, so `"[ ]x"` and `"[y]"` stay literal `(None, original)`.
fn split_task_marker(text: &str) -> (Option<bool>, &str) {
    for (pat, val) in [("[ ]", false), ("[x]", true), ("[X]", true)] {
        if let Some(rest) = text.strip_prefix(pat) {
            if rest.is_empty() {
                return (Some(val), "");
            }
            if let Some(after) = rest.strip_prefix(' ') {
                return (Some(val), after);
            }
        }
    }
    (None, text)
}

/// If a GFM pipe table starts at `lines[i]` — a `|`-bearing line immediately
/// followed by a valid delimiter row — parse it and return the block plus the
/// number of source lines consumed. Returns `None` (consuming nothing) when it
/// isn't a real table, so a paragraph like `a | b` is never misread.
///
/// The delimiter row is the gate: if it isn't all valid `:?-+:?` cells the whole
/// candidate is rejected before any line is consumed.
fn parse_table_at(lines: &[&str], i: usize) -> Option<(MdBlock, usize)> {
    let header_line = lines.get(i)?;
    if !header_line.contains('|') {
        return None;
    }
    let delim_line = lines.get(i + 1)?;
    let aligns = parse_alignments(&split_table_row(delim_line))?;
    let headers = split_table_row(header_line);
    if headers.is_empty() {
        return None;
    }

    // Body rows: subsequent non-blank `|`-bearing lines.
    let mut rows = Vec::new();
    let mut j = i + 2;
    while let Some(l) = lines.get(j) {
        if l.trim().is_empty() || !l.contains('|') {
            break;
        }
        rows.push(split_table_row(l));
        j += 1;
    }

    Some((
        MdBlock::Table {
            headers,
            aligns,
            rows,
        },
        j - i,
    ))
}

/// Split one table row into trimmed cells, dropping the empty cells created by
/// the surrounding `|`. `"| a | b |"` and `"a | b"` both yield `["a", "b"]`.
/// (Escaped `\|` and pipes inside `code` are out of scope.)
fn split_table_row(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    t.split('|').map(|c| c.trim().to_string()).collect()
}

/// Parse a delimiter row's cells into alignments, or `None` if any cell isn't a
/// valid `:?-+:?` separator (≥1 dash). Doubles as the "is this a table?" gate.
fn parse_alignments(cells: &[String]) -> Option<Vec<Align>> {
    if cells.is_empty() {
        return None;
    }
    cells
        .iter()
        .map(|c| {
            let c = c.trim();
            let left = c.starts_with(':');
            let right = c.ends_with(':');
            let core = c.trim_start_matches(':').trim_end_matches(':');
            if core.is_empty() || !core.chars().all(|ch| ch == '-') {
                return None;
            }
            Some(match (left, right) {
                (true, true) => Align::Center,
                (false, true) => Align::Right,
                _ => Align::Left,
            })
        })
        .collect()
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
            let cells = spans_to_cells(&inline_spans(text, style, theme));
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
            checked,
            text,
            indent,
        } => {
            let indent = (*indent).min(8);
            // Marker is a truth table over (checked, ordered).
            let marker = match (checked, ordered) {
                (Some(true), _) => "[x] ".to_string(),
                (Some(false), _) => "[ ] ".to_string(),
                (None, Some(num)) => format!("{num}. "),
                (None, None) => "\u{2022} ".to_string(),
            };
            let marker_color = match checked {
                Some(true) => theme.success,
                _ => theme.accent,
            };
            // Completed items recede so the eye lands on what's left.
            let text_color = if *checked == Some(true) {
                theme.muted
            } else {
                theme.fg
            };
            let prefix_w = indent + display_width(&marker);
            let inner = width.saturating_sub(prefix_w).max(1);
            let cells = spans_to_cells(&inline_spans(text, Style::default().fg(text_color), theme));
            let pad = " ".repeat(indent);
            let first = Span::styled(format!("{pad}{marker}"), Style::default().fg(marker_color));
            let cont = Span::styled(" ".repeat(prefix_w), Style::default());
            with_prefix(wrap_cells(&cells, inner, false), first, cont)
        }
        MdBlock::CodeBlock { lang, lines } => {
            render_code_block(lang.as_deref(), lines, width, theme, syntax_set, syntect_theme)
        }
        MdBlock::Table {
            headers,
            aligns,
            rows,
        } => render_table(headers, aligns, rows, width, theme),
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

// ── Inline parsing ───────────────────────────────────────────────────

/// Parse inline `code`, `**bold**`, `*italic*`, `~~strikethrough~~`, and
/// `[text](url)` links out of `text`, styling the rest with `base`.
/// Unmatched/space-flanked delimiters stay literal.
fn inline_spans(text: &str, base: Style, theme: &Theme) -> Vec<Span<'static>> {
    // Inline `code`: a pink foreground on a shaded card, with one space of
    // padding inside the card on each side (`[ code ]`, GitHub-style) so it
    // reads as a distinct chip and not just tinted text. The padding spaces
    // carry the card colour too.
    let code_style = Style::default().fg(theme.code_fg).bg(theme.code_bg);
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
                // Pad with NBSP (not a regular space) so the wrapper never
                // breaks the chip at its padding — it only wraps on 0x20.
                spans.push(Span::styled(
                    format!("\u{a0}{}\u{a0}", collect(&chars, i + 1, j)),
                    code_style,
                ));
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
        } else if c == '['
            && let Some(link) = parse_link_at(&chars, i)
        {
            flush(&mut buf, &mut spans, base);
            let link_style = base.fg(theme.info).add_modifier(Modifier::UNDERLINED);
            if link.text.is_empty() || link_text_matches_url(&link.text, &link.url) {
                // Empty or self-titled link: show the URL once, styled.
                spans.push(Span::styled(link.url, link_style));
            } else {
                // Link text (which may itself contain inline markup) plus the
                // URL in a recessive, footnote-like parenthetical so the
                // destination stays visible/copyable in a non-clickable TUI.
                spans.extend(inline_spans(&link.text, link_style, theme));
                spans.push(Span::styled(
                    format!(" ({})", link.url),
                    Style::default().fg(theme.muted),
                ));
            }
            i = link.next_i;
            continue;
        } else if c == '~'
            && i + 2 < n
            && chars[i + 1] == '~'
            && !chars[i + 2].is_whitespace()
            && let Some(j) = find_close_strike(&chars, i + 2)
        {
            // Strikethrough `~~text~~`: crossed out AND muted, so the
            // "removed/deprecated" meaning survives terminals that ignore the
            // SGR 9 (crossed-out) escape. (A single `~` stays literal; a `~~~`
            // run at line start is a code fence, handled before inline.)
            flush(&mut buf, &mut spans, base);
            spans.push(Span::styled(
                collect(&chars, i + 2, j),
                base.fg(theme.muted).add_modifier(Modifier::CROSSED_OUT),
            ));
            i = j + 2;
            continue;
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

/// Find the closing `~~` at or after `from` (right-flanking: preceded by a
/// non-space, with non-empty content). Mirrors [`find_close_bold`].
fn find_close_strike(chars: &[char], from: usize) -> Option<usize> {
    let n = chars.len();
    let mut k = from;
    while k + 1 < n {
        if chars[k] == '~' && chars[k + 1] == '~' && k > from && !chars[k - 1].is_whitespace() {
            return Some(k);
        }
        k += 1;
    }
    None
}

/// A parsed `[text](url)` inline link.
struct Link {
    /// The raw link text (may itself contain inline markup).
    text: String,
    url: String,
    /// Index just past the closing `)`.
    next_i: usize,
}

/// Parse a `[text](url)` link whose `[` is at `chars[i]`. Returns `None` when
/// the link is not well-formed (no `]`, no immediately-following `(`, or no
/// closing `)`), so the caller leaves the `[` literal.
///
/// Deliberate simplification: the first `)` closes the URL, so URLs containing
/// a literal `)` (e.g. some Wikipedia links) aren't supported — the remainder
/// falls back to literal text rather than panicking.
fn parse_link_at(chars: &[char], i: usize) -> Option<Link> {
    let text_end = find_char_from(chars, i + 1, ']')?;
    let url_open = text_end + 1;
    if chars.get(url_open) != Some(&'(') {
        return None;
    }
    let url_end = find_char_from(chars, url_open + 1, ')')?;
    Some(Link {
        text: collect(chars, i + 1, text_end),
        url: collect(chars, url_open + 1, url_end),
        next_i: url_end + 1,
    })
}

/// Index of the first `target` char at or after `from`, if any.
fn find_char_from(chars: &[char], from: usize, target: char) -> Option<usize> {
    (from..chars.len()).find(|&k| chars[k] == target)
}

/// Whether the link text is effectively its own URL, so the URL needn't be
/// repeated in parentheses. Compared case-insensitively, ignoring a trailing
/// slash (so `[https://x/](https://x)` collapses).
fn link_text_matches_url(text: &str, url: &str) -> bool {
    let norm = |s: &str| s.trim_end_matches('/').to_ascii_lowercase();
    norm(text) == norm(url)
}

// ── Tables ───────────────────────────────────────────────────────────

/// Render a GFM table borderless: a bold header row, a horizontal rule, then
/// aligned body rows with columns separated by two spaces. Box-drawing borders
/// are intentionally omitted — they cost too much width in the narrow summary
/// column. Over-wide cells are truncated with `…`. (A future refinement could
/// fall back to a `key: value` list when even truncation can't fit.)
fn render_table(
    headers: &[String],
    aligns: &[Align],
    rows: &[Vec<String>],
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let ncols = headers.len();
    if ncols == 0 {
        return vec![Line::from("")];
    }

    // Normalise alignments and body rows to exactly `ncols` columns so the
    // render side never indexes past the header column count.
    let aligns: Vec<Align> = (0..ncols)
        .map(|k| aligns.get(k).copied().unwrap_or(Align::Left))
        .collect();
    let rows: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            let mut row: Vec<String> = r.iter().take(ncols).cloned().collect();
            row.resize(ncols, String::new());
            row
        })
        .collect();

    let widths = fit_col_widths(&natural_col_widths(headers, &rows, theme), width);

    let header_style = Style::default().fg(theme.accent).add_modifier(Modifier::BOLD);
    let body_style = Style::default().fg(theme.fg);

    let mut out = Vec::with_capacity(rows.len() + 2);
    out.push(render_table_row(
        headers,
        &widths,
        &aligns,
        header_style,
        width,
        theme,
    ));
    // Rule under the header, clamped to the panel width.
    let rule_w = (widths.iter().sum::<usize>() + 2 * ncols.saturating_sub(1)).min(width);
    out.push(Line::from(Span::styled(
        "\u{2500}".repeat(rule_w),
        Style::default().fg(theme.muted),
    )));
    for row in &rows {
        out.push(render_table_row(
            row,
            &widths,
            &aligns,
            body_style,
            width,
            theme,
        ));
    }
    out
}

/// Natural width of each column = max rendered (markup-stripped) display width
/// over the header and every body cell.
fn natural_col_widths(headers: &[String], rows: &[Vec<String>], theme: &Theme) -> Vec<usize> {
    let mut w: Vec<usize> = headers.iter().map(|h| rendered_width(h, theme)).collect();
    for row in rows {
        for (k, cell) in row.iter().enumerate() {
            if let Some(col) = w.get_mut(k) {
                *col = (*col).max(rendered_width(cell, theme));
            }
        }
    }
    w
}

/// Display width of `text` after inline markup is stripped, so column widths
/// match what actually renders (not the raw `**bold**` source).
fn rendered_width(text: &str, theme: &Theme) -> usize {
    cells_width(&spans_to_cells(&inline_spans(text, Style::default(), theme)))
}

/// Shrink natural column widths so the row (cells + 2-space separators) fits
/// `width`. Repeatedly trims the widest column by one (not proportional —
/// overkill for a rare wide table); every column keeps at least 1 column.
fn fit_col_widths(natural: &[usize], width: usize) -> Vec<usize> {
    let ncols = natural.len();
    if ncols == 0 {
        return vec![];
    }
    let seps = 2 * (ncols - 1);
    let avail = width.saturating_sub(seps).max(ncols); // >= 1 per column
    let mut w: Vec<usize> = natural.iter().map(|&x| x.max(1).min(avail)).collect();
    while w.iter().sum::<usize>() > avail {
        let maxw = *w.iter().max().unwrap();
        if maxw <= 1 {
            break; // can't shrink further; the final clip guards the width bound
        }
        let idx = w.iter().position(|&x| x == maxw).unwrap();
        w[idx] -= 1;
    }
    w
}

/// Render one table row into a `Line`: each cell fitted to its column width and
/// alignment, columns joined by two spaces, then the whole hard-clipped to
/// `width` as a final guard for degenerate (tiny) widths.
fn render_table_row(
    cells: &[String],
    widths: &[usize],
    aligns: &[Align],
    base: Style,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    let mut row: Vec<Cell> = Vec::new();
    for (k, &col_w) in widths.iter().enumerate() {
        if k > 0 {
            row.push(Cell { ch: ' ', style: base });
            row.push(Cell { ch: ' ', style: base });
        }
        let text = cells.get(k).map(String::as_str).unwrap_or("");
        let align = aligns.get(k).copied().unwrap_or(Align::Left);
        row.extend(fit_cell(text, col_w, align, base, theme));
    }
    cells_to_line(&clip_cells(row, width))
}

/// Fit `text` into exactly `col_w` display columns: render its inline markup,
/// truncate (with a trailing `…`) when too wide, then pad per `align`. Returns
/// cells whose total display width is `col_w` (empty when `col_w` is 0).
/// Works on `Cell`s, so truncation never splits a multibyte char.
fn fit_cell(text: &str, col_w: usize, align: Align, base: Style, theme: &Theme) -> Vec<Cell> {
    if col_w == 0 {
        return Vec::new();
    }
    let cells = truncate_cells(spans_to_cells(&inline_spans(text, base, theme)), col_w);
    let pad = col_w.saturating_sub(cells_width(&cells));
    let space = |n: usize| -> Vec<Cell> {
        (0..n).map(|_| Cell { ch: ' ', style: base }).collect()
    };
    match align {
        Align::Left => {
            let mut out = cells;
            out.extend(space(pad));
            out
        }
        Align::Right => {
            let mut out = space(pad);
            out.extend(cells);
            out
        }
        Align::Center => {
            let left = pad / 2;
            let mut out = space(left);
            out.extend(cells);
            out.extend(space(pad - left));
            out
        }
    }
}

/// Truncate `cells` to at most `max_w` display columns, appending `…` (in the
/// last kept cell's style) only when content was actually cut. Truncates by
/// display width on char boundaries, so multibyte/CJK content never panics.
fn truncate_cells(cells: Vec<Cell>, max_w: usize) -> Vec<Cell> {
    if cells_width(&cells) <= max_w {
        return cells;
    }
    if max_w == 0 {
        return Vec::new();
    }
    let budget = max_w - 1; // reserve one column for the ellipsis
    let mut out: Vec<Cell> = Vec::new();
    let mut w = 0;
    for cell in cells {
        let cw = char_width(cell.ch);
        if w + cw > budget {
            break;
        }
        out.push(cell);
        w += cw;
    }
    let style = out.last().map(|c| c.style).unwrap_or_default();
    out.push(Cell {
        ch: '\u{2026}',
        style,
    });
    out
}

/// Hard-clip `cells` to at most `width` display columns (no ellipsis). The final
/// guard that keeps every table line within the width bound even when the column
/// math can't fit (e.g. a 4-column table in a 3-wide panel).
fn clip_cells(cells: Vec<Cell>, width: usize) -> Vec<Cell> {
    let mut out = Vec::new();
    let mut w = 0;
    for cell in cells {
        let cw = char_width(cell.ch);
        if w + cw > width {
            break;
        }
        out.push(cell);
        w += cw;
    }
    out
}

/// Total display width of a cell slice.
fn cells_width(cells: &[Cell]) -> usize {
    cells.iter().map(|c| char_width(c.ch)).sum()
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
                MdBlock::ListItem { ordered: None, checked: None, text: "a".to_string(), indent: 0 },
                MdBlock::ListItem { ordered: None, checked: None, text: "b".to_string(), indent: 0 },
                MdBlock::ListItem { ordered: Some("1".to_string()), checked: None, text: "c".to_string(), indent: 0 },
                MdBlock::ListItem { ordered: Some("2".to_string()), checked: None, text: "d".to_string(), indent: 0 },
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
        // Inline code is padded with NBSP into a pink-on-card chip; match on the
        // trimmed content rather than the exact padded string.
        assert!(
            spans
                .iter()
                .any(|s| s.content.trim_matches('\u{a0}') == "git"
                    && s.style.fg == Some(theme.code_fg)
                    && s.style.bg == Some(theme.code_bg))
        );
        assert_eq!(joined(&spans), "use \u{a0}git\u{a0} now");
    }

    #[test]
    fn unclosed_inline_delimiters_stay_literal() {
        let (theme, _, _) = fixtures();
        let base = Style::default().fg(theme.fg);
        for input in [
            "a `b", "a *b", "a **b", "*", "`", "a ~~b", "~~", "~", "a ~ b", "~/foo",
            "a ~~ b ~~ c",
        ] {
            let spans = inline_spans(input, base, &theme);
            assert_eq!(joined(&spans), input, "input {input:?} should be literal");
        }
    }

    #[test]
    fn strikethrough_is_styled_and_muted() {
        let (theme, _, _) = fixtures();
        let base = Style::default().fg(theme.fg);
        let spans = inline_spans("keep ~~drop~~ this", base, &theme);
        assert_eq!(joined(&spans), "keep drop this");
        let struck = spans.iter().find(|s| s.content == "drop").unwrap();
        assert!(struck.style.add_modifier.contains(Modifier::CROSSED_OUT));
        assert_eq!(struck.style.fg, Some(theme.muted));
    }

    #[test]
    fn strikethrough_does_not_nest_inner_markup() {
        // Like bold/italic, strikethrough emits its content literally (no
        // nesting) — but inline markup OUTSIDE the strike still works.
        let (theme, _, _) = fixtures();
        let base = Style::default().fg(theme.fg);
        let spans = inline_spans("~~old **bold**~~ and **real**", base, &theme);
        assert!(
            spans
                .iter()
                .any(|s| s.content == "old **bold**"
                    && s.style.add_modifier.contains(Modifier::CROSSED_OUT)),
            "struck run keeps `**` literal"
        );
        assert!(
            spans
                .iter()
                .any(|s| s.content == "real" && s.style.add_modifier.contains(Modifier::BOLD)),
            "bold outside the strike still applies"
        );
    }

    #[test]
    fn bare_tilde_run_is_fence_not_strikethrough() {
        // `~~~` at line start is a code fence, parsed before inline strike.
        assert!(matches!(
            parse_blocks("~~~\ncode\n~~~").as_slice(),
            [MdBlock::CodeBlock { .. }]
        ));
    }

    // ── Task checkboxes ──

    #[test]
    fn task_checkboxes_parse() {
        assert_eq!(
            parse_blocks("- [ ] todo\n- [x] done\n- [X] also\n1. [ ] num"),
            vec![
                MdBlock::ListItem { ordered: None, checked: Some(false), text: "todo".to_string(), indent: 0 },
                MdBlock::ListItem { ordered: None, checked: Some(true), text: "done".to_string(), indent: 0 },
                MdBlock::ListItem { ordered: None, checked: Some(true), text: "also".to_string(), indent: 0 },
                MdBlock::ListItem { ordered: Some("1".to_string()), checked: Some(false), text: "num".to_string(), indent: 0 },
            ]
        );
    }

    #[test]
    fn non_checkboxes_stay_plain_items() {
        // Malformed markers must NOT become checkboxes; text is preserved verbatim.
        for (input, want_text) in [
            ("- [y] thing", "[y] thing"),
            ("- [] thing", "[] thing"),
            ("- [ ]nospace", "[ ]nospace"),
            ("- [ x] thing", "[ x] thing"),
        ] {
            assert_eq!(
                parse_blocks(input),
                vec![MdBlock::ListItem {
                    ordered: None,
                    checked: None,
                    text: want_text.to_string(),
                    indent: 0,
                }],
                "{input:?}"
            );
        }
    }

    #[test]
    fn empty_task_checkbox_parses() {
        assert_eq!(
            parse_blocks("- [ ]"),
            vec![MdBlock::ListItem {
                ordered: None,
                checked: Some(false),
                text: String::new(),
                indent: 0,
            }]
        );
    }

    #[test]
    fn checkbox_renders_within_width() {
        // At widths comfortably above the `[x] ` marker, lines stay in bounds.
        // (Like all list items, a width narrower than the marker can't be
        // honoured — that degenerate case is covered by `never_panics`.)
        for width in [8usize, 20, 40] {
            for line in render("- [x] done\n- [ ] あいうえお task", width) {
                assert!(display_width(&line_text(&line)) <= width);
            }
        }
    }

    // ── Tables ──

    #[test]
    fn table_parses_headers_aligns_rows() {
        assert_eq!(
            parse_blocks("| h1 | h2 |\n| --- | :--: |\n| a | b |\n| c | d |"),
            vec![MdBlock::Table {
                headers: vec!["h1".to_string(), "h2".to_string()],
                aligns: vec![Align::Left, Align::Center],
                rows: vec![
                    vec!["a".to_string(), "b".to_string()],
                    vec!["c".to_string(), "d".to_string()],
                ],
            }]
        );
    }

    #[test]
    fn pipe_paragraph_is_not_a_table() {
        // No delimiter row → not a table; no source line is eaten.
        assert_eq!(
            parse_blocks("a | b\nc | d"),
            vec![
                MdBlock::Paragraph("a | b".to_string()),
                MdBlock::Paragraph("c | d".to_string()),
            ]
        );
        // Header-looking line at EOF with no delimiter.
        assert_eq!(
            parse_blocks("| h1 | h2 |"),
            vec![MdBlock::Paragraph("| h1 | h2 |".to_string())]
        );
        // Delimiter with zero dashes is not a delimiter.
        assert_eq!(
            parse_blocks("| a | b |\n| : | : |").len(),
            2,
            "no-dash second line means two paragraphs, not a table"
        );
    }

    #[test]
    fn table_cell_splitting_normalizes_outer_pipes() {
        assert_eq!(split_table_row("| a | b |"), vec!["a", "b"]);
        assert_eq!(split_table_row("a | b"), vec!["a", "b"]);
        assert_eq!(split_table_row("| a | b"), vec!["a", "b"]);
        assert_eq!(split_table_row("a | b |"), vec!["a", "b"]);
    }

    #[test]
    fn table_renders_within_width_and_truncates() {
        // Header + rule + 2 body rows = 4 lines, all within width.
        let table = "| name | id |\n| --- | --: |\n| alice | 1 |\n| bob | 22 |";
        for width in [0usize, 1, 2, 3, 8, 20, 80] {
            let lines = render(table, width);
            for line in &lines {
                assert!(
                    display_width(&line_text(line)) <= width.max(1),
                    "table line exceeds width {width}"
                );
            }
        }
    }

    #[test]
    fn table_cell_truncation_never_splits_multibyte() {
        // Force CJK / accented cells below their content width — must not panic
        // and must respect the width bound.
        let table = "| name |\n| ---- |\n| café |\n| 日本語テスト |\n| 🧑‍🤝‍🧑x |";
        for width in [1usize, 2, 3, 4, 5, 6, 10] {
            for line in render(table, width) {
                assert!(display_width(&line_text(&line)) <= width.max(1));
            }
        }
    }

    #[test]
    fn table_alignment_does_not_change_cell_width() {
        // Same content under each alignment yields identical column widths.
        let mk = |delim: &str| render(&format!("| h |\n| {delim} |\n| ab |"), 20);
        let widths: Vec<usize> = ["---", ":--", "--:", ":-:"]
            .iter()
            .map(|d| line_text(&mk(d)[2]).trim_end().chars().count())
            .collect();
        // Left/right/center pad differently but the trimmed body content is "ab".
        for d in ["---", ":--", "--:", ":-:"] {
            let body = line_text(&mk(d)[2]);
            assert!(body.contains("ab"), "alignment {d} lost content");
        }
        // The full (untrimmed) row width is identical across alignments.
        let full: Vec<usize> = ["---", ":--", "--:", ":-:"]
            .iter()
            .map(|d| display_width(&line_text(&mk(d)[2])))
            .collect();
        assert!(full.iter().all(|&w| w == full[0]), "row widths differ: {full:?}");
        let _ = widths;
    }

    #[test]
    fn table_ragged_rows_are_normalized() {
        // Short and long rows render without panic, padded/truncated to header.
        let table = "| a | b |\n| - | - |\n| 1 |\n| 1 | 2 | 3 |";
        let lines = render(table, 40);
        // header + rule + 2 rows.
        assert_eq!(lines.len(), 4);
    }

    #[test]
    fn links_render_text_and_url() {
        let (theme, _, _) = fixtures();
        let base = Style::default().fg(theme.fg);

        // Link text shown, URL kept in a recessive parenthetical.
        let spans = inline_spans("see [the docs](https://example.com) now", base, &theme);
        assert_eq!(joined(&spans), "see the docs (https://example.com) now");
        assert!(
            spans.iter().any(|s| s.content == "the docs"
                && s.style.add_modifier.contains(Modifier::UNDERLINED)),
            "link text is underlined"
        );

        // Inline markup inside the link text is still styled.
        let spans = inline_spans("[**bold** link](https://x.io)", base, &theme);
        assert!(
            spans
                .iter()
                .any(|s| s.content == "bold" && s.style.add_modifier.contains(Modifier::BOLD)),
            "emphasis inside link text is preserved"
        );
        assert!(joined(&spans).contains("(https://x.io)"));
    }

    #[test]
    fn self_titled_and_empty_links_show_url_once() {
        let (theme, _, _) = fixtures();
        let base = Style::default().fg(theme.fg);

        let spans = inline_spans("[https://x.com](https://x.com)", base, &theme);
        assert_eq!(joined(&spans), "https://x.com");

        // Trailing-slash / case differences still collapse.
        let spans = inline_spans("[https://x.com/](https://x.com)", base, &theme);
        assert_eq!(joined(&spans), "https://x.com");

        let spans = inline_spans("[](https://x.com)", base, &theme);
        assert_eq!(joined(&spans), "https://x.com");
    }

    #[test]
    fn malformed_links_stay_literal() {
        let (theme, _, _) = fixtures();
        let base = Style::default().fg(theme.fg);
        for input in ["[text]", "[text](", "[text(url)", "a [b] c", "["] {
            let spans = inline_spans(input, base, &theme);
            assert_eq!(joined(&spans), input, "{input:?} should stay literal");
        }
    }

    #[test]
    fn link_preserves_trailing_text() {
        let (theme, _, _) = fixtures();
        let base = Style::default().fg(theme.fg);
        let spans = inline_spans("[a](b)c", base, &theme);
        assert_eq!(joined(&spans), "a (b)c");
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
            "[",
            "[](",
            "[]()",
            "[x](y",
            "[**](http://ünïcode.example/path)",
            "~~",
            "~~~~",
            "a ~~b~~ c",
            "- [ ]",
            "- [x] あ",
            "|",
            "||",
            "| |",
            "|---|",
            "| a |\n|---|",
            "| 日本 | 🧑‍🤝‍🧑 |\n| :-: | --: |\n| あいうえお | x |",
            "a | b",
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
    fn code_block_is_highlighted_and_carded() {
        let (theme, ss, st) = fixtures();
        let lines = render_markdown("```rust\nlet x = 1;\n```", 40, &theme, &ss, &st);
        // Padding rows above and below the code, each filled to full width with
        // the card background.
        assert!(lines.len() >= 3);
        for edge in [&lines[0], &lines[lines.len() - 1]] {
            assert!(display_width(&line_text(edge)) == 40, "pad row fills width");
            assert!(
                edge.spans.iter().all(|s| s.style.bg == Some(theme.code_bg)),
                "pad row carries the card background"
            );
        }
        // The content row sits between the pads: card background under every
        // span, and syntect splits it into multiple styled spans.
        let content = &lines[1];
        assert!(line_text(content).contains("let x = 1;"));
        assert!(content.spans.iter().all(|s| s.style.bg == Some(theme.code_bg)));
        assert!(content.spans.len() > 2);
        // The whole card fills the width edge to edge.
        assert_eq!(display_width(&line_text(content)), 40);
    }

    #[test]
    fn inline_code_sits_on_card_background() {
        let (theme, _, _) = fixtures();
        let base = Style::default().fg(theme.fg);
        let spans = inline_spans("run `cargo test` now", base, &theme);
        let code = spans
            .iter()
            .find(|s| s.content.trim_matches('\u{a0}') == "cargo test")
            .unwrap();
        assert_eq!(code.style.bg, Some(theme.code_bg));
    }

    #[test]
    fn top_level_headings_get_an_underline_rule() {
        // H1/H2 render their text plus a full-width rule; H3+ do not.
        let h1 = render("# Title", 20);
        assert_eq!(h1.len(), 2, "heading + rule");
        assert!(line_text(&h1[1]).chars().all(|c| c == '\u{2500}'));
        assert_eq!(display_width(&line_text(&h1[1])), 20);

        let h3 = render("### Sub", 20);
        assert_eq!(h3.len(), 1, "no rule under H3");
    }

    #[test]
    fn headings_get_a_colour_bar_and_level_colour() {
        let (theme, _, _) = fixtures();
        // The first span of a heading is the solid colour bar; its colour and
        // the heading text's colour track the level.
        for (src, color) in [
            ("# H1", theme.accent),
            ("## H2", theme.info),
            ("### H3", theme.success),
        ] {
            let lines = render(src, 30);
            let bar = &lines[0].spans[0];
            assert_eq!(bar.content.as_ref(), "\u{2503} ");
            assert_eq!(bar.style.fg, Some(color), "bar colour for {src:?}");
            // The text after the bar carries the same level colour, bolded.
            let text = &lines[0].spans[1];
            assert_eq!(text.style.fg, Some(color));
            assert!(text.style.add_modifier.contains(Modifier::BOLD));
        }
    }

    #[test]
    fn apply_background_fills_only_bare_spans() {
        let (theme, ss, st) = fixtures();
        let mut lines =
            render_markdown("text with `code`", 40, &theme, &ss, &st);
        let bg = theme.comment_preview_bg;
        apply_background(&mut lines, bg);
        for line in &lines {
            for span in &line.spans {
                // Plain text gains the tint; the inline-code card keeps its own.
                assert!(span.style.bg == Some(bg) || span.style.bg == Some(theme.code_bg));
            }
        }
    }

    #[test]
    fn markdown_cache_matches_fresh_and_invalidates_on_change() {
        let (theme, ss, st) = fixtures();
        let cache = MarkdownCache::new();
        let texts = |ls: &[Line]| ls.iter().map(line_text).collect::<Vec<_>>();

        // Cached output equals a fresh render.
        let fresh = render_markdown("a `b` c", 30, &theme, &ss, &st);
        let first = cache.render("id1", "a `b` c", 30, &theme, &ss, &st);
        assert_eq!(texts(&fresh), texts(&first));

        // A cache hit (same id/body/width) returns the same content.
        let second = cache.render("id1", "a `b` c", 30, &theme, &ss, &st);
        assert_eq!(texts(&first), texts(&second));

        // Changing the body re-renders (different output under the same id).
        let changed = cache.render("id1", "totally different text", 30, &theme, &ss, &st);
        assert_ne!(texts(&first), texts(&changed));

        // Changing the width re-wraps.
        let narrow = cache.render("id2", "the quick brown fox jumps", 8, &theme, &ss, &st);
        let wide = cache.render("id2", "the quick brown fox jumps", 40, &theme, &ss, &st);
        assert_ne!(narrow.len(), wide.len());
    }
}
