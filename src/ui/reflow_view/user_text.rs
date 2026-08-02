//! user ターン向けのフル幅背景ブロック描画（実測）。Claude Code のライブ UI は user の
//! プロンプトの背後の行全体を塗る。マーカーとテキストだけではない — このモジュールは
//! それをここで再現する。
//!
//! user 入力はパースすべき文章ではなく生のテキストなので、Markdown レンダラを完全に
//! バイパスし（[build](super::build) を参照）、代わりにこのモジュールが単語単位で
//! 折り返す。Markdown の段落詰め直しのようにリフローするのではなく、元のテキストの
//! 改行を1つずつ独立した折り返し行として保持する。

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::glyphs::MARKER_COLS;
use super::helpers::pad_glyph_to;

/// user ターンのテキストの右側、ブロック内に確保しておく背景のみのパディングのカラム数。
/// 実測: 幅60では本文は57カラム（60 - MARKER_COLS - 1）で、カラム59は背景だけのセルに
/// なる。幅100では97になる。assistant のテキストにはこのような予約は無く、
/// width - MARKER_COLS で折り返すので、これは user ブロック固有のものである。
const USER_TRAILING_PAD: usize = 1;

/// user ターンのテキストブロックを1つ、フル幅の背景行として描画する: グリフは最初の行
/// だけ（継続行は他すべてのブロック種別のガターと同じく2スペースの空白インデント）、
/// 本文は width - MARKER_COLS - USER_TRAILING_PAD で単語単位に折り返した上で、
/// 背景がパネル端まで届くよう width - MARKER_COLS まで詰め戻す。marker_style と
/// body_style はあらかじめ背景色を持っている必要がある — この関数はテキスト内容だけを
/// 供給し、パレットの制御は呼び出し側が行う（tool_lines::ToolStyles と対をなす形）。
pub(crate) fn render_user_text(
    text: &str,
    width: usize,
    glyph: &str,
    marker_style: Style,
    body_style: Style,
) -> Vec<Line<'static>> {
    // 本文カラムの塗る幅（背景はパネル端まで届く）……
    let body_width = width.saturating_sub(MARKER_COLS);
    // ……だがテキストはその1カラム手前で止める。
    let wrap_width = body_width.saturating_sub(USER_TRAILING_PAD);
    let marker_prefix = pad_glyph_to(glyph, MARKER_COLS);
    let cont_prefix = " ".repeat(MARKER_COLS);

    wrap_plain_text(text, wrap_width)
        .into_iter()
        .enumerate()
        .map(|(i, body)| {
            let prefix = if i == 0 {
                marker_prefix.clone()
            } else {
                cont_prefix.clone()
            };
            // 本文をカラム予算いっぱいまでパディングし、背景色が最後のグリフで止まらず
            // 行全体を塗るようにする — bg() だけを設定した Style は、span がカバーして
            // いないカラムを塗らないため。
            let padded_body = pad_to_width(&body, body_width);
            Line::from(vec![
                Span::styled(prefix, marker_style),
                Span::styled(padded_body, body_style),
            ])
        })
        .collect()
}

/// テキストを width 表示カラム（unicode_width で計測）まで貪欲に単語折り返しする。まず
/// 既存の改行で分割してから処理するので、元のテキスト自身の改行は詰め直した段落へ
/// 折り込まれることなく、独立した折り返し行として残る。
///
/// width より広い単語はあふれさせず、カラム境界で強制的に分割する。実測: 幅60で
/// W×150 は 57 / 57 / 36 に分かれる。（これは ui::walkthrough_pane::wrap_text とは
/// 異なる挙動で、そちらはそのような単語をあふれさせる — ここでは Claude Code との
/// 一致を優先している。）分割はグラフェムクラスタ単位で歩き、1クラスタが2行にまたがって
/// 分かれることはない。また、現在行を先に吐き出すのではなく、まず現在行の残りを
/// 埋めてから分割する: ⎿ Read <非常に長いパス> という注釈で実測したところ、パスは
/// Read の行から始まり、カラム境界で改行される。
pub(crate) fn wrap_plain_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for source_line in text.split('\n') {
        if source_line.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut current = String::new();
        let mut current_w = 0usize;
        for word in source_line.split_whitespace() {
            let word_w = UnicodeWidthStr::width(word);
            if word_w > width {
                // 吐き出す前に、まず現在行の残りを埋める。⎿ Read <長いパス> という
                // 注釈で実測したところ、パスは Read の行から始まりカラム境界で
                // 改行されるので、動詞だけが単独で行に残ることはない。
                if !current.is_empty() {
                    if current_w + 1 < width {
                        current.push(' ');
                        current_w += 1;
                    } else {
                        out.push(std::mem::take(&mut current));
                        current_w = 0;
                    }
                }
                for cluster in word.graphemes(true) {
                    let cw = UnicodeWidthStr::width(cluster);
                    if current_w + cw > width && !current.is_empty() {
                        out.push(std::mem::take(&mut current));
                        current_w = 0;
                    }
                    current.push_str(cluster);
                    current_w += cw;
                }
                continue;
            }
            let candidate_w = if current.is_empty() {
                word_w
            } else {
                current_w + 1 + word_w
            };
            if candidate_w > width && !current.is_empty() {
                out.push(std::mem::take(&mut current));
                current_w = 0;
            }
            if !current.is_empty() {
                current.push(' ');
                current_w += 1;
            }
            current.push_str(word);
            current_w += word_w;
        }
        out.push(current);
    }
    out
}

/// s の末尾にスペースを足し、ちょうど width 表示カラムを満たすまでパディングする。
/// すでに width 以上ある場合は変更しない。
pub(crate) fn pad_to_width(s: &str, width: usize) -> String {
    let w = UnicodeWidthStr::width(s);
    if w >= width {
        s.to_string()
    } else {
        let mut out = s.to_string();
        for _ in 0..(width - w) {
            out.push(' ');
        }
        out
    }
}
