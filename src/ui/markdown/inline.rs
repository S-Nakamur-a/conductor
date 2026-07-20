//! Inline span parsing: `code`, `**bold**`, `*italic*`, `~~strikethrough~~`,
//! and `[text](url)` links out of a block's text.

use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use crate::theme::Theme;

/// Parse inline `code`, `**bold**`, `*italic*`, `~~strikethrough~~`, and
/// `[text](url)` links out of `text`, styling the rest with `base`.
/// Unmatched/space-flanked delimiters stay literal.
pub(crate) fn inline_spans(text: &str, base: Style, theme: &Theme) -> Vec<Span<'static>> {
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
