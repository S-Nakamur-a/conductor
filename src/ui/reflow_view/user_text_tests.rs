//! [super::user_text] のテスト — user ターンのフル幅背景ブロック: 単語折り返し、
//! カラムパディング、マーカー/継続行のレイアウト。

use ratatui::style::{Color, Style};
use unicode_width::UnicodeWidthStr;

use super::glyphs::MARKER_COLS;
use super::palette;
use super::user_text::{pad_to_width, render_user_text, wrap_plain_text};

// wrap_plain_text

#[test]
fn wrap_fits_on_one_line_unchanged() {
    assert_eq!(wrap_plain_text("hello world", 20), vec!["hello world"]);
}

#[test]
fn wrap_breaks_at_word_boundaries() {
    assert_eq!(
        wrap_plain_text("one two three four", 9),
        vec!["one two", "three", "four"]
    );
}

#[test]
fn wrap_preserves_source_newlines_as_independent_lines() {
    // 幅の予算を十分に下回る短い元の行が2つ — Markdown の文章のように1つの
    // リフローされた段落へ結合されるのではなく、別々の出力行のまま残る必要がある。
    assert_eq!(
        wrap_plain_text("first line\nsecond line", 40),
        vec!["first line", "second line"]
    );
}

#[test]
fn wrap_preserves_blank_source_lines() {
    assert_eq!(wrap_plain_text("a\n\nb", 10), vec!["a", "", "b"]);
}

#[test]
fn wrap_overlong_single_word_is_hard_split() {
    // Claude Code に対して実測: Wx150 を57カラムの予算で折り返すと 57 / 57 / 36 に
    // なるので、分割不能な連続文字はあふれさせるのではなくカラム境界で切られる
    // （あふれさせるのは markdown 側の折り返しの挙動だが、ここでは
    // 一致を優先する）。
    assert_eq!(
        wrap_plain_text("supercalifragilistic", 5),
        vec!["super", "calif", "ragil", "istic"]
    );
}

#[test]
fn wrap_hard_split_matches_the_measured_native_shape() {
    let chunks = wrap_plain_text(&"W".repeat(150), 57);
    let widths: Vec<usize> = chunks.iter().map(|c| c.chars().count()).collect();
    assert_eq!(widths, vec![57, 57, 36]);
}

#[test]
fn wrap_hard_split_never_breaks_a_full_width_glyph() {
    // 予算5に2カラムのグリフ: 1行につき2文字、半分だけのグリフになる行は無い。
    let chunks = wrap_plain_text(&"あ".repeat(5), 5);
    for c in &chunks {
        assert!(
            unicode_width::UnicodeWidthStr::width(c.as_str()) <= 5,
            "{c:?}"
        );
    }
    assert_eq!(chunks.concat(), "あ".repeat(5));
}

// pad_to_width

#[test]
fn pad_short_string_fills_with_trailing_spaces() {
    let padded = pad_to_width("hi", 5);
    assert_eq!(padded, "hi   ");
    assert_eq!(UnicodeWidthStr::width(padded.as_str()), 5);
}

#[test]
fn pad_string_already_at_width_unchanged() {
    assert_eq!(pad_to_width("hello", 5), "hello");
}

#[test]
fn pad_string_wider_than_target_unchanged() {
    assert_eq!(pad_to_width("hello world", 5), "hello world");
}

// render_user_text

fn marker_style() -> Style {
    Style::default()
        .fg(palette::USER_MARKER_FG)
        .bg(palette::USER_BG)
}

fn body_style() -> Style {
    Style::default().fg(palette::USER_TEXT).bg(palette::USER_BG)
}

#[test]
fn first_line_gets_the_marker_continuation_lines_get_blank_indent() {
    let lines = render_user_text(
        "one two three four five six seven",
        12,
        "\u{276f}",
        marker_style(),
        body_style(),
    );
    assert!(
        lines.len() > 1,
        "expected the body to wrap onto multiple lines"
    );
    assert_eq!(lines[0].spans[0].content, "\u{276f} ");
    assert_eq!(lines[1].spans[0].content, "  ");
}

#[test]
fn every_line_is_padded_to_the_full_panel_width_for_the_background() {
    let width = 20;
    let lines = render_user_text("short", width, "\u{276f}", marker_style(), body_style());
    for line in &lines {
        let total: usize = line
            .spans
            .iter()
            .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
            .sum();
        assert_eq!(
            total, width,
            "line {line:?} must fill the full width for its background"
        );
    }
}

#[test]
fn marker_and_body_spans_carry_the_background_color() {
    let lines = render_user_text("hi", 10, "\u{276f}", marker_style(), body_style());
    let line = &lines[0];
    assert_eq!(line.spans[0].style.bg, Some(Color::Rgb(55, 55, 55)));
    assert_eq!(line.spans[1].style.bg, Some(Color::Rgb(55, 55, 55)));
}

#[test]
fn source_newlines_survive_as_separate_lines_each_with_their_own_gutter_slot() {
    let lines = render_user_text(
        "first\nsecond",
        20,
        "\u{276f}",
        marker_style(),
        body_style(),
    );
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].spans[0].content, "\u{276f} ");
    assert_eq!(lines[1].spans[0].content, "  ");
}

#[test]
fn body_wraps_at_width_minus_marker_cols() {
    // width=10 なら body_width = 10 - MARKER_COLS(2) = テキスト用に8カラム残る。
    let lines = render_user_text(
        "abcdefgh ijkl",
        10,
        "\u{276f}",
        marker_style(),
        body_style(),
    );
    assert!(
        lines.len() >= 2,
        "8-col budget must force a wrap: {lines:?}"
    );
    // このテストが依存する定数をガードしておく。MARKER_COLS が変わったとき、
    // 誤った予算のまま黙って通ってしまうのではなく、はっきり失敗させるため。
    assert_eq!(MARKER_COLS, 2);
}

// グラフェムクラスタの幅計算
//
// これらはコードを眺めて見つけたものではなく、コーパススイープで見つかったもの:
// 1文字ずつの合計は文字列全体の幅とどちらの方向にもずれることがあり、どちらにせよ
// 折り返した行は本来の予算と一致しなくなる。

#[test]
fn emoji_presentation_sequence_counts_as_two_columns() {
    // ⚠ 単体は1カラム、⚠ + U+FE0F は2カラム。1文字ずつ合計すると基底文字しか
    // 見えない（セレクタは幅0のため）ので、行が1カラムぶん広くなってしまう —
    // まさにパネル幅の不変条件が捕まえるはみ出しである。
    let warn = "\u{26a0}\u{fe0f}";
    assert_eq!(unicode_width::UnicodeWidthStr::width(warn), 2);

    let wrapped = wrap_plain_text(&warn.repeat(5), 4);
    for line in &wrapped {
        assert!(
            unicode_width::UnicodeWidthStr::width(line.as_str()) <= 4,
            "{line:?} exceeds the budget"
        );
    }
    assert_eq!(wrapped.concat(), warn.repeat(5), "no cluster was dropped");
}

#[test]
fn zwj_sequence_is_never_split() {
    // family 絵文字は2カラムだが7文字ある。その間で分割すると計測を誤るうえに、
    // 画面上にシーケンスの半分だけが残ってしまう。
    let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}";
    let wrapped = wrap_plain_text(&family.repeat(3), 4);
    assert_eq!(wrapped.concat(), family.repeat(3));
    for line in &wrapped {
        assert!(
            unicode_width::UnicodeWidthStr::width(line.as_str()) <= 4,
            "{line:?}"
        );
    }
}
