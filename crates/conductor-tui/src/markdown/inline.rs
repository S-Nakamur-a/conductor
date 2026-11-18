//! インラインのスパン解析。

use std::sync::Arc;

use conductor_core::claude_log::urls::{is_url_char, starts_link, trim_url};
use conductor_core::theme::Theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use unicode_segmentation::UnicodeSegmentation;

use super::Flavor;
use super::wrap::Cell;
use crate::link::is_fullwidth_end;

/// span の字がどこを開くか。
#[derive(Clone)]
enum Opens {
    /// 地の文。素の URL はこの字の並びから探す。
    Text,
    Code,
    Link(Arc<str>),
    /// ` (url)` の url の字だけが開く。
    LinkSuffix(Arc<str>),
}

/// inline_spans と同じ字を、押したときに開く先と一緒にセルで返す。
pub(crate) fn inline_cells(text: &str, base: Style, theme: &Theme, flavor: Flavor) -> Vec<Cell> {
    let mut cells = Vec::new();
    let mut text_cells = Vec::new();
    for (span, opens) in inline_parts(text, base, theme, flavor) {
        let graphemes: Vec<&str> = span.content.graphemes(true).collect();
        let count = graphemes.len();
        for (k, g) in graphemes.into_iter().enumerate() {
            let link = match &opens {
                Opens::Link(url) => Some(url.clone()),
                Opens::LinkSuffix(url) if (2..count - 1).contains(&k) => Some(url.clone()),
                _ => None,
            };
            text_cells.push(matches!(opens, Opens::Text));
            cells.push(Cell {
                text: g.to_string(),
                style: span.style,
                link,
            });
        }
    }
    mark_bare_urls(&mut cells, &text_cells);
    cells
}

/// 字から探す推測 (link::find の 1 行分) と同じ規則で、地の文の素の URL に開く先を付ける。
fn mark_bare_urls(cells: &mut [Cell], text_cells: &[bool]) {
    let chars: Vec<char> = cells
        .iter()
        .map(|c| {
            let mut chars = c.text.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => c,
                _ => char::REPLACEMENT_CHARACTER,
            }
        })
        .collect();
    let text: String = chars.iter().collect();
    let at: Vec<usize> = text
        .char_indices()
        .map(|(at, _)| at)
        .chain([text.len()])
        .collect();
    let mut i = 0;
    while i < chars.len() {
        let after_break = i == 0 || !is_url_char(chars[i - 1]);
        if !text_cells[i] || !starts_link(after_break, &text[at[i]..]) {
            i += 1;
            continue;
        }
        let end = (i..chars.len())
            .find(|&k| !text_cells[k] || !is_url_char(chars[k]))
            .unwrap_or(chars.len());
        let run = &text[at[i]..at[end]];
        let url = trim_url(run);
        let closed = run[url.len()..].contains([')', ']', '}']);
        let ends_here = chars
            .get(end)
            .is_none_or(|&c| c.is_ascii() || is_fullwidth_end(c));
        if closed || ends_here {
            let url: Arc<str> = Arc::from(url);
            for cell in &mut cells[i..i + url.len()] {
                cell.link = Some(url.clone());
            }
        }
        i = end;
    }
}

/// text からインラインの code、**bold**、*italic*、~~strikethrough~~、
/// [text](url) 形式のリンクを解析し、残りは base のスタイルで表示する。
/// 対応する閉じ記号がない、または前後に空白が接するデリミタはそのまま文字として残す。
///
/// flavor が影響するのはインラインコードだけ: Rich UI では色付きのカード
/// （[ code ] のように）にパディングして表示する。Claude のトランスクリプトでは
/// theme.info 色のプレーンテキストとして、パディングも背景もなしで表示し、実物の
/// Claude Code に合わせている。
pub(crate) fn inline_spans(
    text: &str,
    base: Style,
    theme: &Theme,
    flavor: Flavor,
) -> Vec<Span<'static>> {
    inline_parts(text, base, theme, flavor)
        .into_iter()
        .map(|(span, _)| span)
        .collect()
}

fn inline_parts(
    text: &str,
    base: Style,
    theme: &Theme,
    flavor: Flavor,
) -> Vec<(Span<'static>, Opens)> {
    let code_style = match flavor {
        Flavor::Rich => Style::default().fg(theme.code_fg).bg(theme.code_bg),
        Flavor::Transcript => Style::default().fg(theme.info),
    };
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut spans: Vec<(Span<'static>, Opens)> = Vec::new();
    let mut buf = String::new();
    let mut i = 0;

    while i < n {
        let c = chars[i];

        if c == '`' {
            if let Some(j) = (i + 1..n).find(|&k| chars[k] == '`')
                && j > i + 1
            {
                flush(&mut buf, &mut spans, base);
                let content = collect(&chars, i + 1, j);
                let styled = match flavor {
                    // 通常のスペースではなく NBSP でパディングする。ラッパーが
                    // パディング部分でチップを分断してしまわないようにするため
                    // （折り返しは 0x20 でしか起きない）。
                    Flavor::Rich => format!("\u{a0}{content}\u{a0}"),
                    Flavor::Transcript => content,
                };
                spans.push((Span::styled(styled, code_style), Opens::Code));
                i = j + 1;
                continue;
            }
        } else if c == '*' {
            if i + 1 < n && chars[i + 1] == '*' {
                if i + 2 < n
                    && !chars[i + 2].is_whitespace()
                    && let Some(j) = find_close_bold(&chars, i + 2)
                {
                    flush(&mut buf, &mut spans, base);
                    spans.push((
                        Span::styled(collect(&chars, i + 2, j), base.add_modifier(Modifier::BOLD)),
                        Opens::Text,
                    ));
                    i = j + 2;
                    continue;
                }
            } else if i + 1 < n
                && !chars[i + 1].is_whitespace()
                && chars[i + 1] != '*'
                && let Some(j) = find_close_italic(&chars, i + 1)
            {
                flush(&mut buf, &mut spans, base);
                spans.push((
                    Span::styled(
                        collect(&chars, i + 1, j),
                        base.add_modifier(Modifier::ITALIC),
                    ),
                    Opens::Text,
                ));
                i = j + 1;
                continue;
            }
        } else if c == '['
            && let Some(link) = parse_link_at(&chars, i)
        {
            flush(&mut buf, &mut spans, base);
            let link_style = base.fg(theme.info).add_modifier(Modifier::UNDERLINED);
            let url: Arc<str> = Arc::from(link.url.as_str());
            if link.text.is_empty() || link_text_matches_url(&link.text, &link.url) {
                spans.push((Span::styled(link.url, link_style), Opens::Link(url)));
            } else {
                spans.extend(
                    inline_spans(&link.text, link_style, theme, flavor)
                        .into_iter()
                        .map(|span| (span, Opens::Link(url.clone()))),
                );
                spans.push((
                    Span::styled(format!(" ({})", link.url), Style::default().fg(theme.muted)),
                    Opens::LinkSuffix(url),
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
            flush(&mut buf, &mut spans, base);
            spans.push((
                Span::styled(
                    collect(&chars, i + 2, j),
                    base.fg(theme.muted).add_modifier(Modifier::CROSSED_OUT),
                ),
                Opens::Text,
            ));
            i = j + 2;
            continue;
        }

        buf.push(c);
        i += 1;
    }

    flush(&mut buf, &mut spans, base);
    if spans.is_empty() {
        spans.push((Span::styled(String::new(), base), Opens::Text));
    }
    spans
}

fn flush(buf: &mut String, spans: &mut Vec<(Span<'static>, Opens)>, style: Style) {
    if !buf.is_empty() {
        spans.push((Span::styled(std::mem::take(buf), style), Opens::Text));
    }
}

fn collect(chars: &[char], start: usize, end: usize) -> String {
    chars[start..end].iter().collect()
}

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

fn find_close_italic(chars: &[char], from: usize) -> Option<usize> {
    (from..chars.len()).find(|&k| chars[k] == '*' && !chars[k - 1].is_whitespace())
}

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

/// [text](url) 形式のインラインリンクを解析した結果。
struct Link {
    /// リンクテキストの生データ（それ自体がインライン記法を含みうる）。
    text: String,
    url: String,
    /// 閉じ記号 ) の直後のインデックス。
    next_i: usize,
}

/// 形式が不正なら None を返し、呼び出し元は [ をそのまま文字として残す。最初の ) を URL の
/// 閉じとみなすので、) を含む URL (一部の Wikipedia など) は非対応。
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

fn find_char_from(chars: &[char], from: usize, target: char) -> Option<usize> {
    (from..chars.len()).find(|&k| chars[k] == target)
}

/// 同じなら括弧書きで URL を繰り返さない。大文字小文字と末尾スラッシュを無視して比べる
/// ([https://x/](https://x) が縮退するように)。
fn link_text_matches_url(text: &str, url: &str) -> bool {
    let norm = |s: &str| s.trim_end_matches('/').to_ascii_lowercase();
    norm(text) == norm(url)
}
