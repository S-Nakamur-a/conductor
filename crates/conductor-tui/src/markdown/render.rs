//! ブロックから Line への変換: MdBlock 1つを、フェンス付きコードブロックの
//! シンタックスハイライトも含めて、装飾付きで折り返し済みの Line 列に変換する。

use conductor_core::theme::Theme;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::Theme as SyntectTheme;
use syntect::parsing::SyntaxSet;

use super::Flavor;
use super::code_colors::render_code_block_transcript;
use super::inline::inline_spans;
use super::parse::MdBlock;
use super::table::render_table;
use super::table_boxed::render_table_boxed;
use super::wrap::{display_width, spans_to_cells, with_prefix, wrap_cells};

pub(crate) fn render_block(
    block: &MdBlock,
    width: usize,
    theme: &Theme,
    syntax_set: &SyntaxSet,
    syntect_theme: &SyntectTheme,
    flavor: Flavor,
) -> Vec<Line<'static>> {
    match block {
        MdBlock::Blank => vec![Line::from("")],
        MdBlock::Rule => vec![Line::from(Span::styled(
            "\u{2500}".repeat(width),
            Style::default().fg(theme.muted),
        ))],
        // Transcript フレーバー: 色バーも下線ルールもなし。太字の本文色テキストのみ
        // （H1 はさらにイタリック+下線が付き、実物の Claude Code に合わせる。H2 以降は
        // 太字のみ）。前後の空行は render 側で付与される。
        MdBlock::Heading { level, text } if flavor == Flavor::Transcript => {
            let mut modifier = Modifier::BOLD;
            if *level == 1 {
                modifier |= Modifier::ITALIC | Modifier::UNDERLINED;
            }
            let style = Style::default().fg(theme.fg).add_modifier(modifier);
            let cells = spans_to_cells(&inline_spans(text, style, theme, flavor));
            wrap_cells(&cells, width, false)
        }
        MdBlock::Heading { level, text } => {
            // 深さごとに色を変え、見出しレベルが一目で分かるようにする。
            let color = match level {
                1 => theme.accent,
                2 => theme.info,
                3 => theme.success,
                4 => theme.warning,
                _ => theme.hint,
            };
            let style = Style::default().fg(color).add_modifier(Modifier::BOLD);
            // 左側の細い色バー（太字の罫線文字の縦線）が見出しをその色に結び付け、
            // 塗りつぶしブロックほどの重さを出さずにセクションを本文から浮き上がらせる。
            let bar = Span::styled(
                "\u{2503} ".to_string(),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            );
            let cont = Span::styled("  ".to_string(), Style::default());
            let inner = width.saturating_sub(2).max(1);
            let cells = spans_to_cells(&inline_spans(text, style, theme, Flavor::Rich));
            let mut out = with_prefix(wrap_cells(&cells, inner, false), bar, cont);
            // GitHub は H1/H2 の下に下線を引く。それに合わせて、見出しの色を帯びた
            // 全幅のルールを引き、セクションが1つの色付きブロックとして読めるようにする。
            if *level <= 2 {
                out.push(Line::from(Span::styled(
                    "\u{2500}".repeat(width),
                    Style::default().fg(Theme::darken(color, 0.55)),
                )));
            }
            out
        }
        MdBlock::Paragraph(text) => {
            let cells = spans_to_cells(&inline_spans(
                text,
                Style::default().fg(theme.fg),
                theme,
                flavor,
            ));
            wrap_cells(&cells, width, false)
        }
        // 実物の Claude Code は引用を dim な ▎ で示し、本文はターミナルのデフォルト色の
        // イタリックで描画する（muted のグレーは付けない。それは Rich 専用の装飾）。
        // 下の Rich 用の分岐に畳み込まず独立させているのは、グリフ・スタイル・本文色の
        // すべてが異なるため。
        MdBlock::Quote(text) if flavor == Flavor::Transcript => {
            let inner = width.saturating_sub(2).max(1);
            let style = Style::default().fg(theme.fg).add_modifier(Modifier::ITALIC);
            let cells = spans_to_cells(&inline_spans(text, style, theme, flavor));
            let bar = Span::styled(
                "\u{258e} ".to_string(),
                Style::default().add_modifier(Modifier::DIM),
            );
            with_prefix(wrap_cells(&cells, inner, false), bar.clone(), bar)
        }
        MdBlock::Quote(text) => {
            let inner = width.saturating_sub(2).max(1);
            let style = Style::default()
                .fg(theme.muted)
                .add_modifier(Modifier::ITALIC);
            let cells = spans_to_cells(&inline_spans(text, style, theme, Flavor::Rich));
            let bar = Span::styled("\u{2502} ".to_string(), Style::default().fg(theme.muted));
            with_prefix(wrap_cells(&cells, inner, false), bar.clone(), bar)
        }
        MdBlock::ListItem {
            ordered,
            checked,
            text,
            indent,
        } => {
            let indent = (*indent).min(8);
            // 箇条書きのグリフ: リッチ UI では •、Claude のトランスクリプトでは -
            // （どちらも表示幅1桁なので、マーカー幅の計算には影響しない）。
            let bullet = match flavor {
                Flavor::Rich => "\u{2022} ",
                Flavor::Transcript => "- ",
            };
            // 実物の Claude Code は GFM のタスクリスト構文を特別扱いしない。
            // - [ ] text / - [x] text というソース行は、チェックボックスがテキストに
            // 残ったまま無装飾の普通の箇条書き項目として表示される。マーカーを本文に
            // 戻し checked を落として、以降にあるこの分岐の Rich 専用スタイリングが
            // 適用されないようにする。
            let (checked, text): (Option<bool>, String) = if flavor == Flavor::Transcript {
                let literal = |mark: &str| {
                    if text.is_empty() {
                        mark.to_string()
                    } else {
                        format!("{mark} {text}")
                    }
                };
                match checked {
                    Some(true) => (None, literal("[x]")),
                    Some(false) => (None, literal("[ ]")),
                    None => (None, text.clone()),
                }
            } else {
                (*checked, text.clone())
            };
            let marker = match (checked, ordered) {
                (Some(true), _) => "[x] ".to_string(),
                (Some(false), _) => "[ ] ".to_string(),
                (None, Some(num)) => format!("{num}. "),
                (None, None) => bullet.to_string(),
            };
            // Rich では箇条書き/番号にアクセント色を付け、transcript では本文色のまま
            // にする（実物の Claude Code CLI と同じ）。完了済みタスクは常に success 色。
            let marker_color = match (checked, flavor) {
                (Some(true), _) => theme.success,
                (_, Flavor::Transcript) => theme.fg,
                (_, Flavor::Rich) => theme.accent,
            };
            // 完了済みの項目は控えめにし、目が残っているものに向くようにする。
            let text_color = if checked == Some(true) {
                theme.muted
            } else {
                theme.fg
            };
            let prefix_w = indent + display_width(&marker);
            let inner = width.saturating_sub(prefix_w).max(1);
            let cells = spans_to_cells(&inline_spans(
                &text,
                Style::default().fg(text_color),
                theme,
                flavor,
            ));
            let pad = " ".repeat(indent);
            let first = Span::styled(format!("{pad}{marker}"), Style::default().fg(marker_color));
            let cont = Span::styled(" ".repeat(prefix_w), Style::default());
            with_prefix(wrap_cells(&cells, inner, false), first, cont)
        }
        MdBlock::CodeBlock { lang, lines } => match flavor {
            Flavor::Rich => render_code_block(
                lang.as_deref(),
                lines,
                width,
                theme,
                syntax_set,
                syntect_theme,
            ),
            Flavor::Transcript => {
                render_code_block_transcript(lang.as_deref(), lines, width, syntax_set)
            }
        },
        MdBlock::Table {
            headers,
            aligns,
            rows,
        } => match flavor {
            Flavor::Rich => render_table(headers, aligns, rows, width, theme),
            Flavor::Transcript => render_table_boxed(headers, rows, width, theme),
        },
    }
}

/// 各行を theme.code_bg で全幅に塗り、左右 1 列ずつ字下げして枠のように見せる (GitHub と
/// 同じやり方)。単語折り返しではなくハードラップして何も隠さない。
fn render_code_block(
    lang: Option<&str>,
    lines: &[String],
    width: usize,
    theme: &Theme,
    syntax_set: &SyntaxSet,
    syntect_theme: &SyntectTheme,
) -> Vec<Line<'static>> {
    let inner = width.saturating_sub(2).max(1);
    let code_bg = theme.code_bg;
    let syntax = lang
        .and_then(|l| syntax_set.find_syntax_by_token(l))
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text());
    let mut highlighter = HighlightLines::new(syntax, syntect_theme);
    let fallback = Style::default().fg(theme.fg).bg(code_bg);

    let pad_row = || {
        Line::from(Span::styled(
            " ".repeat(width),
            Style::default().bg(code_bg),
        ))
    };

    let mut out = vec![pad_row()];
    for raw in lines {
        let expanded = raw.replace('\t', "    ");
        let with_nl = format!("{expanded}\n");
        let spans: Vec<Span<'static>> = match highlighter.highlight_line(&with_nl, syntax_set) {
            Ok(ranges) => ranges
                .into_iter()
                .map(|(style, piece)| {
                    // すべてのトークンの下にカードの背景色を強制し、syntect のテーマに
                    // 関わらず全体が1つの面として見えるようにする。
                    let st = syntect_tui::translate_style(style)
                        .unwrap_or_default()
                        .bg(code_bg);
                    Span::styled(piece.trim_end_matches('\n').to_string(), st)
                })
                .filter(|s| !s.content.is_empty())
                .collect(),
            Err(_) => vec![Span::styled(expanded.clone(), fallback)],
        };
        let cells = spans_to_cells(&spans);
        let wrapped = if cells.is_empty() {
            vec![Line::from("")]
        } else {
            wrap_cells(&cells, inner, true)
        };
        for line in wrapped {
            out.push(frame_code_row(line, width, code_bg));
        }
    }
    out.push(pad_row());
    out
}

/// 先頭の字下げ・span 列・末尾のパディングの全セルに code_bg を持たせ、行が width 桁ぶん
/// 一色で埋まるようにする。
fn frame_code_row(line: Line<'static>, width: usize, code_bg: Color) -> Line<'static> {
    let inset = Style::default().bg(code_bg);
    let mut spans: Vec<Span<'static>> = Vec::with_capacity(line.spans.len() + 2);
    spans.push(Span::styled(" ".to_string(), inset));
    let mut used = 1usize;
    for span in line.spans {
        used += display_width(&span.content);
        spans.push(span);
    }
    if used < width {
        spans.push(Span::styled(" ".repeat(width - used), inset));
    }
    Line::from(spans)
}
