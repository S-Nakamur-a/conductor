//! インラインのスパン解析: ブロックのテキストから code、**bold**、*italic*、
//! ~~strikethrough~~、[text](url) 形式のリンクを取り出す。

use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use crate::theme::Theme;

use super::MarkdownFlavor;

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
    flavor: MarkdownFlavor,
) -> Vec<Span<'static>> {
    let code_style = match flavor {
        // 影付きのカードの上にピンクの前景色を乗せ、カードの内側に左右1スペース分の
        // パディングを入れる（[ code ] のような GitHub 風）。単に色が付いたテキストでは
        // なく、独立したチップとして読めるようにするため。パディングのスペースにも
        // カードの色を乗せる。
        MarkdownFlavor::Rich => Style::default().fg(theme.code_fg).bg(theme.code_bg),
        // 実物の Claude Code: theme.info 色のテキストで、カードもパディングもなし。
        MarkdownFlavor::Transcript => Style::default().fg(theme.info),
    };
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let mut i = 0;

    while i < n {
        let c = chars[i];

        if c == '`' {
            // インラインコード: 次のバッククォートと対応させる。中身は何でもよい。
            if let Some(j) = (i + 1..n).find(|&k| chars[k] == '`')
                && j > i + 1
            {
                flush(&mut buf, &mut spans, base);
                let content = collect(&chars, i + 1, j);
                let styled = match flavor {
                    // 通常のスペースではなく NBSP でパディングする。ラッパーが
                    // パディング部分でチップを分断してしまわないようにするため
                    // （折り返しは 0x20 でしか起きない）。
                    MarkdownFlavor::Rich => format!("\u{a0}{content}\u{a0}"),
                    // 実物の Claude Code はインラインコードの周りにパディングを入れない。
                    MarkdownFlavor::Transcript => content,
                };
                spans.push(Span::styled(styled, code_style));
                i = j + 1;
                continue;
            }
        } else if c == '*' {
            if i + 1 < n && chars[i + 1] == '*' {
                // 太字: 開き記号 ** の直後は非空白でなければならない。
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
                // 斜体: 開き記号 * の直後は非空白。
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
                // 空、またはテキストが URL 自身と同じリンク: URL を1回だけ装飾して表示する。
                spans.push(Span::styled(link.url, link_style));
            } else {
                // リンクテキスト（それ自体がインライン記法を含みうる）に続けて、URL を
                // 控えめな脚注風の括弧書きで表示する。クリックできない TUI でも行き先を
                // 見える・コピーできる状態に保つため。
                spans.extend(inline_spans(&link.text, link_style, theme, flavor));
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
            // 打ち消し線 ~~text~~: CROSSED_OUT と muted 色の両方を適用する。ターミナルが
            // SGR 9（打ち消し線）エスケープを無視しても「削除済み/非推奨」という意味が
            // 伝わるようにするため。（単独の ~ はそのまま文字として残る。行頭の ~~~ は
            // コードフェンスとしてインライン処理より前に扱われる。）
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

/// from 以降で閉じ記号 ** を探す（右接: 直前が非空白で、中身が空でないこと）。
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

/// from 以降で閉じ記号 * を探す（右接: 直前が非空白であること）。
fn find_close_italic(chars: &[char], from: usize) -> Option<usize> {
    (from..chars.len()).find(|&k| chars[k] == '*' && !chars[k - 1].is_whitespace())
}

/// from 以降で閉じ記号 ~~ を探す（右接: 直前が非空白で、中身が空でないこと）。
/// find_close_bold と対になる。
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

/// from 以降で最初に現れる target 文字のインデックス（あれば）。
fn find_char_from(chars: &[char], from: usize, target: char) -> Option<usize> {
    (from..chars.len()).find(|&k| chars[k] == target)
}

/// 同じなら括弧書きで URL を繰り返さない。大文字小文字と末尾スラッシュを無視して比べる
/// ([https://x/](https://x) が縮退するように)。
fn link_text_matches_url(text: &str, url: &str) -> bool {
    let norm = |s: &str| s.trim_end_matches('/').to_ascii_lowercase();
    norm(text) == norm(url)
}
