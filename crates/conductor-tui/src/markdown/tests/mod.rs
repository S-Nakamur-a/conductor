//! markdown レンダラのテストスイートが共有するテスト用フィクスチャ。
//! parsing（ブロック/インライン/テーブルの解析）と rendering（折り返し・
//! 堅牢性・コードブロック/transcript の描画）、code_colors（transcript の
//! コードブロックの配色）に関心事ごとに分けてある。

use conductor_core::theme::Theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::highlighting::ThemeSet;

use super::inline::inline_spans;
use super::parse::{Align, split_table_row};
use super::wrap::display_width;
use super::*;

mod code_colors;
mod parsing;
mod rendering;

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

fn render_rich(text: &str, width: usize) -> Vec<Line<'static>> {
    let (theme, ss, st) = fixtures();
    render(text, width, &theme, &ss, &st, Flavor::Rich)
}

fn render_transcript(text: &str, width: usize) -> Vec<Line<'static>> {
    let (theme, ss, st) = fixtures();
    render(text, width, &theme, &ss, &st, Flavor::Transcript)
}

#[test]
fn is_markdown_pathは拡張子だけを大小無視で見る() {
    for path in [
        "README.md",
        "docs/plan.markdown",
        "README.MD",
        "a/b/c.Markdown",
    ] {
        assert!(is_markdown_path(path), "{path} should be markdown");
    }
    // .mdx / .mdown / 拡張子なしの README にはあえて一致させない。
    for path in ["src/main.rs", "page.mdx", "README", "mdbook.toml", ""] {
        assert!(!is_markdown_path(path), "{path} should not be markdown");
    }
}
