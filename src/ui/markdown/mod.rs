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
//!   summary column. A cell too wide for its column **wraps** onto extra lines
//!   (the row grows to its tallest cell) rather than truncating — in a table
//!   the cut text is usually the point of the row, and nothing in these views
//!   can reveal it afterwards.
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
//! private, individually testable helper, split across submodules: [`parse`]
//! (line-oriented block parsing), [`inline`] (inline emphasis/links/code),
//! [`render`] (block-to-`Line` rendering), [`table`] (GFM table layout), and
//! [`wrap`] (display-width-aware span wrapping).

mod inline;
mod parse;
mod render;
mod table;
mod wrap;

use parse::{MdBlock, parse_blocks};
use render::render_block;

use ratatui::style::Color;
use ratatui::text::Line;
use syntect::highlighting::Theme as SyntectTheme;
use syntect::parsing::SyntaxSet;

use crate::theme::Theme;

/// Which visual dialect to render in. The renderer is shared between conductor's
/// own rich UI (change summaries, review comments, walkthrough) and the Claude
/// Code transcript overlay (the reflow scroll-up view); the two want different
/// list-marker and heading chrome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkdownFlavor {
    /// Conductor UI: `•` bullets in the accent colour; headings get a coloured
    /// left bar and (H1/H2) a full-width underline rule.
    Rich,
    /// Claude Code transcript: `-` bullets in the body colour; headings render as
    /// bold body-colour text with a blank line above and below and no bar or
    /// rule — matching how the real Claude Code CLI prints markdown.
    Transcript,
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
    render_markdown_flavored(
        text,
        width,
        theme,
        syntax_set,
        syntect_theme,
        MarkdownFlavor::Rich,
    )
}

/// [`render_markdown`] with an explicit [`MarkdownFlavor`]. The 5-arg
/// [`render_markdown`] is this with [`MarkdownFlavor::Rich`]; the Claude
/// transcript overlay passes [`MarkdownFlavor::Transcript`].
pub fn render_markdown_flavored(
    text: &str,
    width: usize,
    theme: &Theme,
    syntax_set: &SyntaxSet,
    syntect_theme: &SyntectTheme,
    flavor: MarkdownFlavor,
) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut out: Vec<Line<'static>> = Vec::new();
    // Track whether the previous block was blank so headings get one (and only
    // one) blank line of breathing room above them — GitHub-style section
    // separation that makes the structure scannable at a glance.
    let mut prev_blank = true;
    // In Transcript flavor a heading also gets a blank line *below* it; if the
    // source already has a blank line next, swallow it so the two don't stack.
    let mut swallow_next_blank = false;
    for block in parse_blocks(text) {
        let is_blank = matches!(block, MdBlock::Blank);
        let is_heading = matches!(block, MdBlock::Heading { .. });
        if is_blank && swallow_next_blank {
            swallow_next_blank = false;
            prev_blank = true;
            continue;
        }
        swallow_next_blank = false;
        if is_heading && !prev_blank {
            out.push(Line::from(""));
        }
        out.extend(render_block(
            &block,
            width,
            theme,
            syntax_set,
            syntect_theme,
            flavor,
        ));
        if is_heading && flavor == MarkdownFlavor::Transcript {
            out.push(Line::from(""));
            swallow_next_blank = true;
            prev_blank = true;
        } else {
            prev_blank = is_blank;
        }
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

    /// Clear all cached entries.
    ///
    /// Called by `App::apply_appearance` after the syntect theme is replaced so
    /// that the next render re-highlights code blocks with the new theme. The
    /// cache fingerprint only tracks the UI theme colour palette; a syntect-only
    /// change (e.g. `[viewer] syntax_theme_file`) would otherwise leave stale
    /// highlighted spans in the cache.
    pub fn clear(&self) {
        self.entries.borrow_mut().clear();
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
        self.render_flavored(
            key,
            body,
            width,
            theme,
            syntax_set,
            syntect_theme,
            MarkdownFlavor::Rich,
        )
    }

    /// [`render`](Self::render) with an explicit [`MarkdownFlavor`]. A given cache
    /// instance is used with a single flavor throughout (conductor's `markdown_cache`
    /// is Rich, the reflow transcript's cache is Transcript), so flavor is not part
    /// of the cache key.
    #[allow(clippy::too_many_arguments)]
    pub fn render_flavored(
        &self,
        key: &str,
        body: &str,
        width: usize,
        theme: &Theme,
        syntax_set: &SyntaxSet,
        syntect_theme: &SyntectTheme,
        flavor: MarkdownFlavor,
    ) -> Vec<Line<'static>> {
        self.ensure(key, body, width, theme, syntax_set, syntect_theme, flavor);
        self.entries.borrow()[key].lines.clone()
    }

    /// Render a scrollable document and return only the visible window:
    /// `(total_lines, clamped_skip, lines[clamped_skip..][..take])`.
    ///
    /// Same caching and invalidation as [`render`](Self::render); it exists
    /// because the Viewer's rendered-markdown mode re-draws a whole file every
    /// frame, where `render`'s clone-the-entire-document cost would scale with
    /// file length instead of with the viewport.
    ///
    /// `skip` is clamped to the last line *here*, where the true total is known.
    /// A caller clamping beforehand would have to use the previous frame's total
    /// — stale exactly when it matters (the document or the wrap width just
    /// changed), which shows up as a blank viewport the user can't scroll out of.
    #[allow(clippy::too_many_arguments)]
    pub fn render_window(
        &self,
        key: &str,
        body: &str,
        width: usize,
        theme: &Theme,
        syntax_set: &SyntaxSet,
        syntect_theme: &SyntectTheme,
        skip: usize,
        take: usize,
    ) -> (usize, usize, Vec<Line<'static>>) {
        self.ensure(
            key,
            body,
            width,
            theme,
            syntax_set,
            syntect_theme,
            MarkdownFlavor::Rich,
        );
        let entries = self.entries.borrow();
        let lines = &entries[key].lines;
        let skip = skip.min(lines.len().saturating_sub(1));
        (
            lines.len(),
            skip,
            lines.iter().skip(skip).take(take).cloned().collect(),
        )
    }

    /// Populate `key`'s entry if absent or stale. On return the entry is
    /// guaranteed present and current, so callers may index it directly.
    #[allow(clippy::too_many_arguments)]
    fn ensure(
        &self,
        key: &str,
        body: &str,
        width: usize,
        theme: &Theme,
        syntax_set: &SyntaxSet,
        syntect_theme: &SyntectTheme,
        flavor: MarkdownFlavor,
    ) {
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
            return;
        }
        let lines = render_markdown_flavored(body, width, theme, syntax_set, syntect_theme, flavor);
        self.entries.borrow_mut().insert(
            key.to_string(),
            CacheEntry {
                body: body.to_string(),
                width,
                lines,
            },
        );
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

#[cfg(test)]
mod tests;
