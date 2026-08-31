//! マーカー・インデント・省略表示のヘルパー群。App から独立してテスト可能な純粋関数で、
//! [build](super::build) がトランスクリプト行のレイアウトに使う。

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

pub(crate) use crate::ui::common::text::truncate_to_width;

use super::glyphs::MARKER_COLS;

/// lines の先頭行に MARKER_COLS 幅のマーカーを付け、継続行すべてに同じ幅の空白インデントを
/// 付ける。
///
/// glyph は unicode_width で幅を測り、先頭スパンとして挿入する前にスペースで
/// ちょうど MARKER_COLS 表示カラムまでパディングする。各行のコンテンツスパンは元のスタイルを
/// 保持する。
pub(crate) fn with_marker(
    lines: Vec<Line<'static>>,
    glyph: &str,
    marker_style: Style,
) -> Vec<Line<'static>> {
    let marker_prefix = pad_glyph_to(glyph, MARKER_COLS);
    let cont_prefix = " ".repeat(MARKER_COLS);

    lines
        .into_iter()
        .enumerate()
        .map(|(i, mut line)| {
            let prefix = if i == 0 {
                Span::styled(marker_prefix.clone(), marker_style)
            } else {
                Span::raw(cont_prefix.clone())
            };
            line.spans.insert(0, prefix);
            line
        })
        .collect()
}

/// glyph の末尾にスペースを足して、ちょうど target_cols 表示カラムを占めるまでパディングする。
/// glyph がすでに target_cols 以上の幅であれば、そのまま返す。
pub(crate) fn pad_glyph_to(glyph: &str, target_cols: usize) -> String {
    let w = UnicodeWidthStr::width(glyph);
    if w >= target_cols {
        glyph.to_string()
    } else {
        let mut s = glyph.to_string();
        for _ in 0..(target_cols - w) {
            s.push(' ');
        }
        s
    }
}

/// parts を indent_cols 個の空白カラムの後ろに並べて1行に組み立てる。結果が width を
/// 超える場合は、切り詰めた単一スパンにフォールバックする。
///
/// 固定フォーマットのサマリ行（Read 3 files (ctrl+o to expand)、
/// Thought for 8s (ctrl+o to expand) など）は、件数の部分だけ太字にするため複数のスパンから
/// 組み立てられている。パネルが狭いと全体を切る必要が出るが、スパンごとに切ると
/// 太字/通常の境界が意味不明な位置に残ってしまう。そのためフォールバックではテキストを
/// 保持しつつスタイルを捨てる。これをしないと行がそのまま width を超えて出力されてしまい、
/// それが corpus sweep で検出しようとしている「にじみ」である。
pub(crate) fn fit_styled_line(
    indent_cols: usize,
    parts: &[(String, Style)],
    width: usize,
) -> Line<'static> {
    let indent = " ".repeat(indent_cols);
    let budget = width.saturating_sub(indent_cols);
    let plain: String = parts.iter().map(|(t, _)| t.as_str()).collect();

    if UnicodeWidthStr::width(plain.as_str()) <= budget {
        let mut spans = Vec::with_capacity(parts.len() + 1);
        spans.push(Span::raw(indent));
        spans.extend(parts.iter().map(|(t, s)| Span::styled(t.clone(), *s)));
        return Line::from(spans);
    }
    let fallback = parts.first().map(|(_, s)| *s).unwrap_or_default();
    Line::from(vec![
        Span::raw(indent),
        Span::styled(truncate_to_width(&plain, budget), fallback),
    ])
}

/// [fit_styled_line] の空白の代わりにマーカーガターへ glyph を置くバージョン。自前のマーカーを
/// 持つ単一行ブロック（⏺ {notice}、✻ Conversation compacted … など）向け。glyph は
/// 先頭パートのスタイルを引き継ぐ。
///
/// fit_styled_line のどちらの分岐もインデントを span 0 に置くので、そこをその場で
/// 置き換えれば、収まったあとの本文には手を加えずに済む。
pub(crate) fn fit_glyph_line(
    glyph: &str,
    parts: &[(String, Style)],
    width: usize,
) -> Line<'static> {
    let mut line = fit_styled_line(MARKER_COLS, parts, width);
    let style = parts.first().map(|(_, s)| *s).unwrap_or_default();
    line.spans[0] = Span::styled(pad_glyph_to(glyph, MARKER_COLS), style);
    line
}
