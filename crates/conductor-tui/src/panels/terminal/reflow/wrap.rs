//! ユーザのターンの全幅背景ブロックと、その素朴な折り返し。
//!
//! ユーザ入力は文章ではなく生のテキストなので markdown を通さず、元の改行を
//! 段落に畳まずそのまま独立した行として残す。

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::style::{MARKER_COLS, pad_glyph_to};

/// 実測: 幅 60 のとき本文は 57 カラムで、最後の 1 カラムは背景だけのセルになる。
const TRAILING_PAD: usize = 1;

/// ユーザのテキストブロックを全幅の背景行にする。背景がパネル端まで届くよう、
/// 本文は折り返し幅より広い body_width まで詰め戻す。
pub(super) fn render_user_text(
    text: &str,
    width: usize,
    glyph: &str,
    marker_style: Style,
    body_style: Style,
) -> Vec<Line<'static>> {
    let body_width = width.saturating_sub(MARKER_COLS);
    let marker = pad_glyph_to(glyph, MARKER_COLS);
    let indent = " ".repeat(MARKER_COLS);

    wrap_plain_text(text, body_width.saturating_sub(TRAILING_PAD))
        .into_iter()
        .enumerate()
        .map(|(i, body)| {
            let prefix = if i == 0 {
                marker.clone()
            } else {
                indent.clone()
            };
            Line::from(vec![
                Span::styled(prefix, marker_style),
                Span::styled(pad_to_width(&body, body_width), body_style),
            ])
        })
        .collect()
}

/// 貪欲な単語折り返し。元の改行で先に割るので、段落として詰め直されることはない。
///
/// width より広い単語は溢れさせずカラム境界で割る (実測: 幅 57 で W×150 は
/// 57 / 57 / 36)。割る前に現在行の残りを埋めるのも実測で、⎿ Read <長いパス> の
/// パスは Read と同じ行から始まる。
pub(super) fn wrap_plain_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for source in text.split('\n') {
        if source.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut line = String::new();
        let mut cols = 0usize;
        for word in source.split_whitespace() {
            let word_cols = UnicodeWidthStr::width(word);
            if word_cols > width {
                if !line.is_empty() {
                    if cols + 1 < width {
                        line.push(' ');
                        cols += 1;
                    } else {
                        out.push(std::mem::take(&mut line));
                        cols = 0;
                    }
                }
                // 書記素クラスタで進める。char 単位だと ZWJ の絵文字が割れる。
                for cluster in word.graphemes(true) {
                    let cw = UnicodeWidthStr::width(cluster);
                    if cols + cw > width && !line.is_empty() {
                        out.push(std::mem::take(&mut line));
                        cols = 0;
                    }
                    line.push_str(cluster);
                    cols += cw;
                }
                continue;
            }
            let joined = if line.is_empty() {
                word_cols
            } else {
                cols + 1 + word_cols
            };
            if joined > width && !line.is_empty() {
                out.push(std::mem::take(&mut line));
                cols = 0;
            }
            if !line.is_empty() {
                line.push(' ');
                cols += 1;
            }
            line.push_str(word);
            cols += word_cols;
        }
        out.push(line);
    }
    out
}

/// ちょうど width カラムになるまで末尾を空白で埋める。bg だけを持つ Style は
/// スパンが覆っていないカラムを塗らないので、背景ブロックにはこれが要る。
pub(super) fn pad_to_width(s: &str, width: usize) -> String {
    let mut out = s.to_string();
    for _ in UnicodeWidthStr::width(s)..width {
        out.push(' ');
    }
    out
}
