//! トランスクリプトのマーカー、配色、行組みの部品。
//!
//! 配色は conductor のテーマではなく Claude Code のダークテーマに固定する。
//! ライブ PTY と読み比べたときに同じ絵に見えることを優先している。

use conductor_core::theme::Theme;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// 左のマーカー溝に割り当てる表示カラム数。
pub(super) const MARKER_COLS: usize = 2;

pub(super) const ASSISTANT_MARKER: &str = "\u{23fa}";
pub(super) const USER_MARKER: &str = "\u{276f}";
pub(super) const TOOL_RESULT_GLYPH: &str = "\u{23bf}";
pub(super) const THINKING_GLYPH: &str = "\u{273b}";
pub(super) const TEAMMATE_GLYPH: &str = "\u{203a}";

/// unicode-width は 1 カラムと数えるのに、多くの端末が 2 カラムで描く字。
/// ❯ と › を入れていないのは、どちらも Emoji presentation を持たない約物で、
/// しかも ❯ は全幅の背景ブロックの中にあり、空セルが背景の切れ込みに見えるため。
const WIDTH_AMBIGUOUS: [char; 3] = ['\u{23fa}', '\u{23bf}', '\u{273b}'];

pub(super) fn is_width_ambiguous(ch: char) -> bool {
    WIDTH_AMBIGUOUS.contains(&ch)
}

pub(super) const CLAUDE: Color = Color::Rgb(215, 119, 87);
/// 端末の既定前景。純白を焼くと、ライブ PTY (vt100 の Default) から
/// スクロールしてきたときにこちらだけ明るく見える。
pub(super) const TEXT: Color = Color::Reset;
pub(super) const SUCCESS: Color = Color::Rgb(78, 186, 101);
pub(super) const ERROR: Color = Color::Rgb(255, 107, 128);
pub(super) const INACTIVE: Color = Color::Rgb(153, 153, 153);
pub(super) const PERMISSION: Color = Color::Rgb(177, 185, 249);
pub(super) const SUBTLE: Color = Color::Rgb(80, 80, 80);
pub(super) const USER_BG: Color = Color::Rgb(55, 55, 55);
pub(super) const USER_MARKER_FG: Color = Color::Rgb(80, 80, 80);
pub(super) const USER_TEXT: Color = Color::Rgb(255, 255, 255);

/// markdown レンダラが読むフィールドだけ Claude の色に差し替えたテーマ。
pub(super) fn markdown_theme(base: &Theme) -> Theme {
    Theme {
        fg: TEXT,
        muted: INACTIVE,
        hint: INACTIVE,
        accent: CLAUDE,
        info: PERMISSION,
        success: SUCCESS,
        error: ERROR,
        warning: Color::Rgb(255, 193, 7),
        border_secondary: SUBTLE,
        code_fg: TEXT,
        code_bg: Color::Rgb(43, 43, 43),
        ..base.clone()
    }
}

pub(super) struct ToolStyles {
    pub marker: Style,
    pub marker_err: Style,
    pub name: Style,
    pub arg: Style,
    pub result: Style,
    pub result_err: Style,
}

pub(super) struct Styles {
    pub assistant: Style,
    /// ユーザのターンは > 接頭辞ではなく全幅の背景ブロック。マーカーも本文も背景色を持つ。
    pub user_marker: Style,
    pub user_body: Style,
    pub result: Style,
    pub thinking: Style,
    pub tools: ToolStyles,
}

impl Default for Styles {
    fn default() -> Self {
        let result = Style::default().fg(INACTIVE);
        Self {
            assistant: Style::default().fg(TEXT),
            user_marker: Style::default().fg(USER_MARKER_FG).bg(USER_BG),
            user_body: Style::default().fg(USER_TEXT).bg(USER_BG),
            result,
            thinking: result.add_modifier(Modifier::ITALIC),
            tools: ToolStyles {
                marker: Style::default().fg(SUCCESS),
                marker_err: Style::default().fg(ERROR),
                name: Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
                // 実測: Write(/tmp/out.txt) の括弧の中は灰色ではなく本文と同色。
                arg: Style::default().fg(TEXT),
                result,
                result_err: Style::default().fg(ERROR),
            },
        }
    }
}

/// glyph の後ろにスペースを足して、ちょうど target_cols を占めさせる。
pub(super) fn pad_glyph_to(glyph: &str, target_cols: usize) -> String {
    let width = UnicodeWidthStr::width(glyph);
    let mut out = glyph.to_string();
    for _ in width..target_cols {
        out.push(' ');
    }
    out
}

/// 先頭行にマーカー、続く行に同じ幅の空白を付ける。
pub(super) fn with_marker(
    lines: Vec<Line<'static>>,
    glyph: &str,
    marker_style: Style,
) -> Vec<Line<'static>> {
    let marker = pad_glyph_to(glyph, MARKER_COLS);
    let indent = " ".repeat(MARKER_COLS);
    lines
        .into_iter()
        .enumerate()
        .map(|(i, mut line)| {
            let prefix = if i == 0 {
                Span::styled(marker.clone(), marker_style)
            } else {
                Span::raw(indent.clone())
            };
            line.spans.insert(0, prefix);
            line
        })
        .collect()
}

/// parts を indent_cols の空白の後ろに並べる。width を超えるならスタイルを捨てて
/// 1 スパンに切り詰める。スパンごとに切ると太字/通常の境目が意味の無い位置に残る。
pub(super) fn fit_styled_line(
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
    let style = parts.first().map(|(_, s)| *s).unwrap_or_default();
    Line::from(vec![
        Span::raw(indent),
        Span::styled(truncate_to_width(&plain, budget), style),
    ])
}

/// [fit_styled_line] の空白の代わりに溝へ字形を置く。自前のマーカーを持つ 1 行ブロック用。
pub(super) fn fit_glyph_line(
    glyph: &str,
    parts: &[(String, Style)],
    width: usize,
) -> Line<'static> {
    let mut line = fit_styled_line(MARKER_COLS, parts, width);
    let style = parts.first().map(|(_, s)| *s).unwrap_or_default();
    line.spans[0] = Span::styled(pad_glyph_to(glyph, MARKER_COLS), style);
    line
}

/// 収まらなければ末尾を省略記号に置き換える。境目は書記素クラスタで決める。
pub(super) fn truncate_to_width(s: &str, max_cols: usize) -> String {
    if max_cols == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(s) <= max_cols {
        return s.to_string();
    }
    let budget = max_cols - 1;
    let mut width = 0usize;
    for (i, cluster) in s.grapheme_indices(true) {
        let cw = UnicodeWidthStr::width(cluster);
        if width + cw > budget {
            return format!("{}\u{2026}", &s[..i]);
        }
        width += cw;
    }
    s.to_string()
}
