//! change-summary ビュー向けの最小限の markdown レンダラ。
//!
//! 意図的に CommonMark 実装にはしていない。summary は短く自筆の PR 説明文のようなものなので、
//! 行単位の小さなパーサで足り、markdown クレートを導入するまでもない。
//!
//! 端末の都合で字形を選んでいる箇所:
//!
//! - リンクは下線付きテキストの後ろに URL を括弧書きで残す。ターミナルではリンクを確実に
//!   クリックできないので、読者がコピーできるようにする。
//! - タスクチェックボックスは ASCII の角括弧 (☐/☑ は East-Asian Ambiguous 幅で CJK 幅の
//!   ターミナルでずれる)。
//! - 打ち消し線は CROSSED_OUT に加えて muted 色も付ける。SGR 9 を無視する端末でも意味が伝わる。
//! - テーブルのセルは切り詰めずに折り返す。切り詰められた文字列こそがその行の要点である
//!   ことが多く、これらのビューには後から全文を確認する手段が無い。

mod code_colors;
mod inline;
mod parse;
mod render;
mod table;
mod table_boxed;
mod wrap;

use std::path::Path;

use conductor_core::theme::Theme;
use ratatui::text::Line;
use syntect::highlighting::Theme as SyntectTheme;
use syntect::parsing::SyntaxSet;

use parse::{MdBlock, parse_blocks};
use render::render_block;

/// どちらの見た目で描画するか。conductor 自身の UI (change summary、レビュー
/// コメント) と Claude Code のトランスクリプト表示では、求める装飾が違う。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    Rich,
    /// 実物の Claude Code CLI が markdown を表示するときの見た目に合わせている。
    Transcript,
}

/// Markdown の text を、width を超えない幅で折り返した装飾付きの行に変換する。
///
/// syntax_set/syntect_theme はフェンス付きコードブロックのハイライトに使われ、
/// 呼び出し元がアプリケーション全体で共有しているインスタンスを渡す想定。
pub fn render(
    text: &str,
    width: usize,
    theme: &Theme,
    syntax_set: &SyntaxSet,
    syntect_theme: &SyntectTheme,
    flavor: Flavor,
) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut out: Vec<Line<'static>> = Vec::new();
    // 見出しの前には必ず空行を 1 つ置く。Transcript では後ろにも置き、元の文書に
    // 続く空行は飲んで二重にしない。
    let mut prev_blank = true;
    let mut swallow_next_blank = false;
    for block in parse_blocks(text) {
        let is_blank = matches!(block, MdBlock::Blank);
        let is_heading = matches!(block, MdBlock::Heading { .. });
        if is_blank && swallow_next_blank {
            swallow_next_blank = false;
            prev_blank = true;
            continue;
        }
        swallow_next_blank = false;
        if is_heading && !prev_blank {
            out.push(Line::from(""));
        }
        out.extend(render_block(
            &block,
            width,
            theme,
            syntax_set,
            syntect_theme,
            flavor,
        ));
        if is_heading && flavor == Flavor::Transcript {
            out.push(Line::from(""));
            swallow_next_blank = true;
            prev_blank = true;
        } else {
            prev_blank = is_blank;
        }
    }
    if out.is_empty() {
        out.push(Line::from(""));
    }
    out
}

/// path が markdown ファイルか。拡張子のみ、大小無視。.mdx / .mdown / 拡張子なしの
/// README にあえて一致させないのは、レンダラが change summary 用の小さな CommonMark
/// サブセットで、広げると黙って誤整形するため。
pub fn is_markdown_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown"))
}

#[cfg(test)]
mod tests;
