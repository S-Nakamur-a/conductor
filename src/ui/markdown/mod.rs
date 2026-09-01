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

use parse::{MdBlock, parse_blocks};
use render::render_block;

use ratatui::style::Color;
use ratatui::text::Line;
use syntect::highlighting::Theme as SyntectTheme;
use syntect::parsing::SyntaxSet;

use crate::theme::Theme;

/// どちらの見た目で描画するか。このレンダラは conductor 自身のリッチな UI
/// （change summary、レビューコメント）と Claude Code のトランスクリプト
/// オーバーレイ（reflow のスクロールアップビュー）の両方で共用されており、両者は
/// リストマーカーと見出しの装飾に異なる見た目を求める。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkdownFlavor {
    /// conductor の UI 向け。箇条書きはアクセント色の•、見出しには色付きの左バーが付き、
    /// H1/H2 にはさらに全幅の下線ルールが付く。
    Rich,
    /// Claude Code のトランスクリプト向け。箇条書きは本文色の-、見出しは上下に空行を挟んだ
    /// 本文色の太字テキストになり、バーや下線は付かない。実物の Claude Code CLI が
    /// markdown を表示するときの見た目に合わせている。
    Transcript,
}

/// Markdown の text を、width を超えない幅で折り返した装飾付きの行に変換する。
///
/// syntax_set/syntect_theme はフェンス付きコードブロックのハイライトに使われ、
/// アプリケーション全体で共有されているインスタンス（App::syntax_set 参照）を渡す想定。
pub fn render_markdown(
    text: &str,
    width: usize,
    theme: &Theme,
    syntax_set: &SyntaxSet,
    syntect_theme: &SyntectTheme,
) -> Vec<Line<'static>> {
    render_markdown_flavored(
        text,
        width,
        theme,
        syntax_set,
        syntect_theme,
        MarkdownFlavor::Rich,
    )
}

/// render_markdown に MarkdownFlavor を明示的に渡せる版。引数5個の render_markdown は
/// これに MarkdownFlavor::Rich を渡したものにあたる。Claude のトランスクリプト
/// オーバーレイは MarkdownFlavor::Transcript を渡す。
pub fn render_markdown_flavored(
    text: &str,
    width: usize,
    theme: &Theme,
    syntax_set: &SyntaxSet,
    syntect_theme: &SyntectTheme,
    flavor: MarkdownFlavor,
) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut out: Vec<Line<'static>> = Vec::new();
    // 直前のブロックが空行だったかを追跡する。見出しの前に空行を1つだけ（かつ必ず1つ）
    // 挟んで余白を作るため。構造が一目で追えるようにする GitHub 流のセクション区切り。
    let mut prev_blank = true;
    // Transcript フレーバーでは見出しの後ろにも空行を入れる。元の文書に元々空行が続く
    // 場合は、それを飲み込んで二重に空行が並ばないようにする。
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
        if is_heading && flavor == MarkdownFlavor::Transcript {
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

/// render_markdown の出力を安定 id ごとにキャッシュする。インラインのスレッドボックス内の
/// コメント/返信本文が毎フレーム再パース・再ハイライトされないようにするため（diff は
/// 60fps で再描画される）。保持するのは背景色を含まない行で、呼び出し元は後から
/// apply_background を適用する（コストは小さい）。本文または折り返し幅が変わるとその
/// エントリを、テーマが変わるとキャッシュ全体を無効化する。
#[derive(Default)]
pub struct MarkdownCache {
    entries: std::cell::RefCell<std::collections::HashMap<String, CacheEntry>>,
    theme_fp: std::cell::Cell<u64>,
}

struct CacheEntry {
    body: String,
    width: usize,
    lines: Vec<Line<'static>>,
}

impl MarkdownCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// キャッシュされたエントリをすべて破棄する。
    ///
    /// syntect のテーマが差し替えられた後、App::apply_appearance から呼ばれ、次の描画で
    /// コードブロックを新しいテーマで再ハイライトさせる。キャッシュのフィンガープリントは
    /// UI テーマの配色パレットしか見ていないため、syntect 側だけの変更（[viewer]
    /// syntax_theme_file など）ではこれを呼ばないとハイライト済みのスパンが古いままキャッシュに
    /// 残ってしまう。
    pub fn clear(&self) {
        self.entries.borrow_mut().clear();
    }

    /// key に対応する本文/幅/テーマが変わっていなければキャッシュ済みの行を返し、
    /// そうでなければ描画して保存する。返す行は背景色を明示的には持たない。
    pub fn render(
        &self,
        key: &str,
        body: &str,
        width: usize,
        theme: &Theme,
        syntax_set: &SyntaxSet,
        syntect_theme: &SyntectTheme,
    ) -> Vec<Line<'static>> {
        self.render_flavored(
            key,
            body,
            width,
            theme,
            syntax_set,
            syntect_theme,
            MarkdownFlavor::Rich,
        )
    }

    /// render に MarkdownFlavor を明示的に渡せる版。1つのキャッシュインスタンスは常に
    /// 単一のフレーバーで使われる（conductor の markdown_cache は Rich、reflow の
    /// トランスクリプト用キャッシュは Transcript）ため、flavor はキャッシュキーには含めない。
    #[allow(clippy::too_many_arguments)]
    pub fn render_flavored(
        &self,
        key: &str,
        body: &str,
        width: usize,
        theme: &Theme,
        syntax_set: &SyntaxSet,
        syntect_theme: &SyntectTheme,
        flavor: MarkdownFlavor,
    ) -> Vec<Line<'static>> {
        self.ensure(key, body, width, theme, syntax_set, syntect_theme, flavor);
        self.entries.borrow()[key].lines.clone()
    }

    /// スクロール可能なドキュメントを描画し、見えている部分の窓だけを返す:
    /// (total_lines, clamped_skip, lines[clamped_skip..][..take])。
    ///
    /// キャッシュと無効化の仕組みは render と同じ。これが別に存在するのは、Viewer の
    /// markdown 描画モードでは毎フレームファイル全体を描き直すため、render のように
    /// ドキュメント全体を clone するコストがビューポートではなくファイル長に比例して
    /// しまうのを避けるため。
    ///
    /// skip はここで、真の総行数が分かっている時点でクランプする。呼び出し元が先に
    /// クランプしようとすると前フレームの総行数を使うことになり、まさに問題になる場面
    /// （ドキュメントや折り返し幅が変わった直後）で古い値になり、ユーザがスクロールで
    /// 抜け出せない空白のビューポートが表示されてしまう。
    #[allow(clippy::too_many_arguments)]
    pub fn render_window(
        &self,
        key: &str,
        body: &str,
        width: usize,
        theme: &Theme,
        syntax_set: &SyntaxSet,
        syntect_theme: &SyntectTheme,
        skip: usize,
        take: usize,
    ) -> (usize, usize, Vec<Line<'static>>) {
        self.ensure(
            key,
            body,
            width,
            theme,
            syntax_set,
            syntect_theme,
            MarkdownFlavor::Rich,
        );
        let entries = self.entries.borrow();
        let lines = &entries[key].lines;
        let skip = skip.min(lines.len().saturating_sub(1));
        (
            lines.len(),
            skip,
            lines.iter().skip(skip).take(take).cloned().collect(),
        )
    }

    /// key のエントリが存在しないか古ければ埋める。呼び出しから戻った時点で、
    /// エントリの存在と最新性が保証されるので、呼び出し元は直接インデックスしてよい。
    #[allow(clippy::too_many_arguments)]
    fn ensure(
        &self,
        key: &str,
        body: &str,
        width: usize,
        theme: &Theme,
        syntax_set: &SyntaxSet,
        syntect_theme: &SyntectTheme,
        flavor: MarkdownFlavor,
    ) {
        // テーマの切り替えはキャッシュ済みスパンに焼き込まれた色を変えてしまうので、
        // フィンガープリントが変わったらエントリを全部破棄する。
        let fp = theme_fingerprint(theme);
        if self.theme_fp.get() != fp {
            self.entries.borrow_mut().clear();
            self.theme_fp.set(fp);
        }
        if let Some(e) = self.entries.borrow().get(key)
            && e.body == body
            && e.width == width
        {
            return;
        }
        let lines = render_markdown_flavored(body, width, theme, syntax_set, syntect_theme, flavor);
        self.entries.borrow_mut().insert(
            key.to_string(),
            CacheEntry {
                body: body.to_string(),
                width,
                lines,
            },
        );
    }
}

/// Markdown 描画に影響するテーマの色を1つの数値に畳み込む。エントリごとにテーマ全体を
/// 保持しなくても、テーマの変化を検知できるようにするため。
fn theme_fingerprint(theme: &Theme) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for c in [
        theme.fg,
        theme.accent,
        theme.info,
        theme.muted,
        theme.success,
        theme.warning,
        theme.hint,
        theme.border_secondary,
        theme.code_bg,
        theme.code_fg,
    ] {
        let bits = match c {
            Color::Rgb(r, g, b) => ((r as u32) << 16) | ((g as u32) << 8) | b as u32,
            _ => u32::MAX,
        };
        bits.hash(&mut h);
    }
    h.finish()
}

/// まだ自前の背景色を持っていないすべての span の背後に bg を塗る。
///
/// [render_markdown] は通常のテキストに背景色を付けずに残す（背後に描かれている面が
/// そのまま透けるように）が、コードには専用の code_bg カードを与える。色付きの面
/// （コメントスレッドボックスの comment_preview_bg など）の上に markdown を描画する
/// 呼び出し元は、これを使って隙間を埋め、コードカードは独自の色味を保ちつつ
/// ブロック全体で背景を統一する。
pub fn apply_background(lines: &mut [Line<'static>], bg: Color) {
    for line in lines {
        for span in &mut line.spans {
            if span.style.bg.is_none() {
                span.style = span.style.bg(bg);
            }
        }
    }
}

#[cfg(test)]
mod tests;
