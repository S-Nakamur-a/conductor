//! markdown レンダラのテストスイートが共有するテスト用フィクスチャ。
//! parsing（ブロック/インライン/テーブルの解析）と rendering（折り返し・
//! 堅牢性・コードブロック/transcript の描画・キャッシュ）に関心事ごとに分けてある。

use super::inline::inline_spans;
use super::parse::{Align, split_table_row};
use super::wrap::display_width;
use super::*;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::highlighting::ThemeSet;

mod parsing;
mod rendering;
mod transcript_code_block_colors;

fn fixtures() -> (Theme, SyntaxSet, SyntectTheme) {
    let theme = Theme::default();
    let syntax_set = two_face::syntax::extra_newlines();
    let syntect_theme = ThemeSet::load_defaults()
        .themes
        .remove("base16-ocean.dark")
        .unwrap();
    (theme, syntax_set, syntect_theme)
}

/// 行内のすべての span の内容を1つの文字列に連結する。
fn line_text(line: &Line) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

fn render(text: &str, width: usize) -> Vec<Line<'static>> {
    let (theme, ss, st) = fixtures();
    render_markdown(text, width, &theme, &ss, &st)
}

fn render_transcript(text: &str, width: usize) -> Vec<Line<'static>> {
    let (theme, ss, st) = fixtures();
    render_markdown_flavored(text, width, &theme, &ss, &st, MarkdownFlavor::Transcript)
}
