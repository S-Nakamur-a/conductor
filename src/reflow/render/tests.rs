//! [super::helpers] のマーカー/インデント/切り詰めヘルパーと、[super::glyphs] の
//! ガター幅の不変条件についてのテスト。

use ratatui::style::{Color, Style};
use ratatui::text::Line;
use unicode_width::UnicodeWidthStr;

use super::glyphs::{
    ASSISTANT_MARKER, MARKER_COLS, TEAMMATE_MESSAGE_GLYPH, THINKING_GLYPH, TOOL_RESULT_GLYPH,
    USER_MARKER,
};
use super::helpers::{pad_glyph_to, with_marker};

#[test]
fn pad_glyph_ascii_pads_to_target() {
    // ">" は幅1カラム。2までパディングすると "> " になるはず。
    assert_eq!(pad_glyph_to(">", 2), "> ");
}

#[test]
fn pad_glyph_already_at_target_unchanged() {
    assert_eq!(pad_glyph_to("=>", 2), "=>");
}

#[test]
fn pad_glyph_wider_than_target_unchanged() {
    assert_eq!(pad_glyph_to("abc", 2), "abc");
}

#[test]
fn pad_glyph_assistant_marker_produces_two_cols() {
    // ⏺ (U+23FA) の unicode_width は1。2までパディングするとスペースが1個付くはず。
    let padded = pad_glyph_to(ASSISTANT_MARKER, MARKER_COLS);
    assert_eq!(UnicodeWidthStr::width(padded.as_str()), MARKER_COLS);
}

#[test]
fn gutter_markers_are_exactly_one_column() {
    // ガターのマーカーは必ず1カラムとして計測されなければならない。すべての行の本文
    // 予算が width - MARKER_COLS として計算されるため。ここでマーカーが1より
    // 大きく計測されると、各トランスクリプト行が1カラム分足りなくなり、最後の文字が
    // パネル端をはみ出してしまう。
    //
    // 「1として計測される」ことと「実際に1として描画される」ことは別である: ⏺/⎿/✻ は
    // 多くの端末やフォントで2カラム幅に描かれる。そのギャップはここでは埋めていない
    // （以前のコメントは U+FE0E のテキスト表示セレクタがそれを埋めていると主張していたが、
    // これらの定数にそのようなセレクタは存在しない）。代わりにグリフの直後のセルを
    // 未書き込みのままにして本文を絶対位置で配置することで対処している。
    // glyphs::WIDTH_AMBIGUOUS_GLYPHS と build::width_risk_hole を参照。
    for (name, m) in [
        ("assistant", ASSISTANT_MARKER),
        ("tool-result", TOOL_RESULT_GLYPH),
        ("thinking", THINKING_GLYPH),
        ("user", USER_MARKER),
        ("teammate-message", TEAMMATE_MESSAGE_GLYPH),
    ] {
        assert_eq!(
            UnicodeWidthStr::width(m),
            1,
            "marker {name} must be exactly 1 display column"
        );
    }
}

#[test]
fn with_marker_prepends_glyph_to_first_line() {
    let style = Style::default().fg(Color::Green);
    let lines = vec![Line::from("hello"), Line::from("world")];
    let result = with_marker(lines, ">", style);
    assert_eq!(result.len(), 2);
    // 最初の行の最初の span がマーカー。
    assert_eq!(result[0].spans[0].content, "> ");
    // 2行目は空白インデントになる。
    assert_eq!(result[1].spans[0].content, "  ");
}

#[test]
fn with_marker_empty_input_returns_empty() {
    let style = Style::default();
    let result = with_marker(vec![], ">", style);
    assert!(result.is_empty());
}

#[test]
fn with_marker_single_line_no_continuation() {
    let style = Style::default();
    let result = with_marker(vec![Line::from("only")], ">", style);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].spans[0].content, "> ");
}
