//! 2 列の組み立てと描画。行は [crate::workspace::Workspace::prepare] で組み、
//! 描画は窓を切り出すだけ。
//!
//! 右の列を歩くのは diff であって項目ではない。行を出しているのは
//! [revidere::ReadingOrder] で、そのループの主語は変更一覧の側。項目が漏らしても
//! 変更行は消えず、最悪でも帯の無い素の diff に退化する。

use conductor_core::diff_state::DiffLineTag;
use conductor_core::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::{Loaded, artifact::importance_color, scope_label};
use crate::panels::viewer::diff::Entry;
use crate::panels::viewer::render::{digit_count, unified_line};
use crate::workspace::Workspace;

/// 左列の重要度ラベルが取る表示幅。一番長い「影響あり」に合わせる。揃えないと、
/// ラベルの長さの違いだけで見出しの左端がぎざぎざになる。
const LABEL_W: usize = 8;

/// 機能への影響の字下げと、ラベル欄の幅。一番長い「確かめる」に合わせてある。
const IMPACT_INDENT: usize = 6;
const IMPACT_LABEL_W: usize = 8;

/// 概要の本文を流し込む最大の幅。端から端まで伸びた 1 行は、折り返した先で
/// 目が戻る場所を見失う。
const READING_W: u16 = 110;

/// 前回からの進みに出すファイル名の数。溢れた分は件数だけ添える。
///
/// 履歴が書き換わったあとは「別々の履歴どうしの全差分」になって数百件になりうる。
/// 全部出すと、この節の下にある概要の 5 欄が画面外へ押し出される。
const SINCE_PREVIOUS_FILES_MAX: usize = 12;

/// 「これが変わったら中身も変わる」入力の指紋。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    pub order_width: u16,
    pub diff_width: u16,
    pub theme: &'static str,
    pub epoch: u64,
}

#[derive(Debug)]
pub struct Rendered {
    pub key: Key,
    pub order_lines: Vec<Line<'static>>,
    /// 画面に出した行 → 項目の番号。見出しの折り返しで 1 項目が何行にもなるので、
    /// クリックした行から割り算では引けない。
    pub item_of_row: Vec<usize>,
    row_of_item: Vec<usize>,
    pub diff_lines: Vec<Line<'static>>,
    /// 項目ごとの、右列での先頭行。
    pub section_rows: Vec<usize>,
    pub overview_lines: Vec<Line<'static>>,
}

impl Rendered {
    /// 選択中の項目が 1/3 の高さに来るよう左列を送る。
    pub fn order_scroll(&self, selected: usize, height: usize) -> usize {
        let anchor = self.row_of_item.get(selected).copied().unwrap_or(0);
        anchor
            .saturating_sub(height / 3)
            .min(self.order_lines.len().saturating_sub(1))
    }
}

pub fn build(key: Key, review: &Loaded, theme: &Theme, tab_width: usize) -> Rendered {
    let (order_lines, item_of_row, row_of_item) =
        order_column(review, theme, inner_width(key.order_width));
    let (diff_lines, section_rows) =
        diff_column(review, theme, inner_width(key.diff_width), tab_width);
    let overview_width = inner_width(key.diff_width.min(READING_W + 2));
    Rendered {
        key,
        order_lines,
        item_of_row,
        row_of_item,
        diff_lines,
        section_rows,
        overview_lines: overview(review, theme, overview_width),
    }
}

fn inner_width(width: u16) -> usize {
    width.saturating_sub(2) as usize
}

pub fn order_title(ws: &Workspace) -> String {
    let panel = &ws.panels.revidere;
    let Some(review) = panel.review() else {
        return " 読む順 ".to_string();
    };
    let count = review.order.sections.len();
    if review.is_complete() {
        format!(" 読む順 {count} 項目  (o: 概要へ) ")
    } else {
        format!(
            " 読む順 {count} 項目  説明の無い変更 {} 件  (o: 概要へ) ",
            review.unexplained()
        )
    }
}

pub fn diff_title(ws: &Workspace) -> String {
    let panel = &ws.panels.revidere;
    let scope = scope_label(panel.scope());
    let Some(review) = panel.review() else {
        return format!(" [{scope}] ");
    };
    if panel.showing_overview() {
        return format!(
            " 概要 [{scope}]  {}..作業ツリー  (d: 項目と diff へ / p: 区間を切り替え) ",
            review.base
        );
    }
    format!(
        " [{scope}] {}..作業ツリー  変更行 {} ",
        review.base,
        review.total_positions()
    )
}

/// 成果物がまだ無いときに 1 列で出す案内。どちらの区間が無いのかを言う — 伏せると、
/// p で切り替えた先が未解析なだけなのに「レビューが消えた」ように読める。
fn empty_lines(ws: &Workspace, theme: &Theme) -> Vec<Line<'static>> {
    let panel = &ws.panels.revidere;
    let text = if panel.is_loading() {
        format!("[{}] のレビューを読み込み中…", scope_label(panel.scope()))
    } else {
        format!(
            "[{}] のレビューはまだ無い — W で解析、p でもう一方の区間へ。",
            scope_label(panel.scope())
        )
    };
    vec![Line::styled(text, Style::default().fg(theme.warning))]
}

pub fn order(frame: &mut Frame, rect: Rect, ws: &Workspace) {
    let inner = crate::list::inner(rect);
    let panel = &ws.panels.revidere;
    let Some(cache) = panel.cache() else {
        return;
    };
    let height = inner.height as usize;
    let scroll = cache.order_scroll(panel.selected(), height);
    let selected_style = Style::default()
        .fg(ws.theme.selected_fg)
        .bg(ws.theme.selected_bg)
        .add_modifier(Modifier::BOLD);
    let lines: Vec<Line> = cache
        .order_lines
        .iter()
        .zip(&cache.item_of_row)
        .skip(scroll)
        .take(height)
        .map(|(line, item)| {
            if *item == panel.selected() {
                line.clone().style(selected_style)
            } else {
                line.clone()
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

pub fn diff(frame: &mut Frame, rect: Rect, ws: &Workspace) {
    let panel = &ws.panels.revidere;
    let inner = crate::list::inner(rect);
    let Some(cache) = panel.cache() else {
        frame.render_widget(Paragraph::new(empty_lines(ws, &ws.theme)), inner);
        return;
    };
    let (source, scroll, area) = if panel.showing_overview() {
        // 余った幅は枠の外に出して中央に置く。枠だけ全幅で伸ばすと、右側の
        // 空きが折り返しの失敗に見える。
        (
            &cache.overview_lines,
            panel.overview_scroll(),
            centered(inner, READING_W),
        )
    } else {
        (&cache.diff_lines, panel.diff_scroll(), inner)
    };
    let lines: Vec<Line> = source
        .iter()
        .skip(scroll)
        .take(area.height as usize)
        .cloned()
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn centered(area: Rect, max_w: u16) -> Rect {
    if area.width <= max_w {
        return area;
    }
    Rect {
        x: area.x + (area.width - max_w) / 2,
        width: max_w,
        ..area
    }
}

/// 見出しは折り返して全部出す。切ると、似た書き出しの項目が見分けられない。
fn order_column(
    review: &Loaded,
    theme: &Theme,
    inner_w: usize,
) -> (Vec<Line<'static>>, Vec<usize>, Vec<usize>) {
    let sections = review.annotations.sections();
    let mut lines = Vec::new();
    let mut item_of_row = Vec::new();
    let mut row_of_item = Vec::with_capacity(review.order.sections.len());
    for (i, placed) in review.order.sections.iter().enumerate() {
        row_of_item.push(lines.len());
        let indent = "  ".repeat(placed.depth);
        let (label, color) = label_of(placed.importance, theme);
        let title = placed
            .section
            .and_then(|s| sections.get(s))
            .map_or("(どの項目でも説明されていない変更)", |s| {
                s.title.as_str()
            });
        let pad = " ".repeat(LABEL_W.saturating_sub(label.width()));
        let width = inner_w.saturating_sub(indent.len() + LABEL_W + 2).max(8);
        for (n, chunk) in wrap(title, width).into_iter().enumerate() {
            let head = if n == 0 {
                format!("{indent}\u{258c}{label}{pad} ")
            } else {
                format!("{indent}\u{258c}{} ", " ".repeat(LABEL_W))
            };
            lines.push(Line::from(vec![
                Span::styled(head, Style::default().fg(color)),
                Span::styled(chunk, Style::default().fg(theme.fg)),
            ]));
            item_of_row.push(i);
        }
        // 項目が在るのに指す行が diff に 1 つも無い状態。黙って消すと「在ると
        // 言った変更が無かった」ことに気付けない。
        if placed.is_empty() {
            lines.push(Line::styled(
                format!("{indent}   (この項目が指す変更が diff に無い)"),
                Style::default().fg(theme.warning),
            ));
            item_of_row.push(i);
        }
    }
    (lines, item_of_row, row_of_item)
}

fn label_of(importance: Option<revidere::Importance>, theme: &Theme) -> (&'static str, Color) {
    match importance {
        Some(importance) => (importance.label_ja(), importance_color(importance)),
        // どの項目でも説明されていない変更。末尾にまとまる。
        None => ("説明なし", theme.muted),
    }
}

fn diff_column(
    review: &Loaded,
    theme: &Theme,
    inner_w: usize,
    tab_width: usize,
) -> (Vec<Line<'static>>, Vec<usize>) {
    let sections = review.annotations.sections();
    let digits = digit_count(max_line_no(review));
    let mut lines = Vec::new();
    let mut section_rows = Vec::with_capacity(review.order.sections.len());

    for placed in &review.order.sections {
        section_rows.push(lines.len());
        let (label, color) = label_of(placed.importance, theme);
        let section = placed.section.and_then(|s| sections.get(s));
        let title = section.map_or("どの項目でも説明されていない変更", |s| {
            s.title.as_str()
        });
        lines.push(Line::styled(
            format!("\u{2500}\u{2500} {label} {title}"),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
        if let Some(section) = section {
            for chunk in wrap(&section.body, inner_w.saturating_sub(2)) {
                lines.push(Line::styled(
                    format!("  {chunk}"),
                    Style::default().fg(theme.fg),
                ));
            }
            // なぜその重要度なのかは全項目必須。誤分類は機械では見つからないが、
            // 理由が読めれば人が見つけられる。
            if let Some(reason) = &section.reason {
                push_hanging(
                    &mut lines,
                    &format!("  なぜ{label}: "),
                    reason,
                    theme.muted,
                    inner_w,
                );
            }
        }
        lines.push(Line::default());

        for block in &placed.blocks {
            let head = if block.hunk.is_empty() {
                format!("  {}", block.path)
            } else {
                format!("  {}  @@ {}", block.path, block.hunk)
            };
            lines.push(Line::styled(
                head,
                Style::default().fg(theme.diff_section_header),
            ));
            if block.whole_file {
                // 行を持たない変更 (バイナリ、モードのみ、純粋な rename)。落とすと
                // 「変更が無かった」と区別が付かなくなる。
                lines.push(Line::styled(
                    "   (行を持たない変更)".to_string(),
                    Style::default().fg(theme.muted),
                ));
            }
            for ordered in &block.lines {
                lines.push(diff_line(
                    &ordered.line,
                    ordered.owned,
                    theme,
                    digits,
                    inner_w,
                    tab_width,
                ));
            }
            lines.push(Line::default());
        }
    }
    (lines, section_rows)
}

fn max_line_no(review: &Loaded) -> usize {
    review
        .order
        .sections
        .iter()
        .flat_map(|placed| &placed.blocks)
        .flat_map(|block| &block.lines)
        .filter_map(|ordered| ordered.line.new_line.or(ordered.line.old_line))
        .max()
        .unwrap_or(0) as usize
}

fn diff_line(
    line: &revidere::DiffLine,
    owned: bool,
    theme: &Theme,
    digits: usize,
    inner_w: usize,
    tab_width: usize,
) -> Line<'static> {
    let (tag, band_color) = match line.tag {
        revidere::Tag::Add => (DiffLineTag::Insert, theme.diff_add),
        revidere::Tag::Del => (DiffLineTag::Delete, theme.diff_del),
        revidere::Tag::Context => (DiffLineTag::Equal, theme.muted),
    };
    let entry = Entry::Line {
        tag,
        old_line_no: line.old_line.map(|n| n as usize),
        new_line_no: line.new_line.map(|n| n as usize),
        content: expand_tabs(&line.text, tab_width),
        inline_segments: Vec::new(),
    };
    let mut rendered = unified_line(&entry, theme, digits, inner_w.saturating_sub(1), 0, None);
    let band = if owned { "\u{258c}" } else { " " };
    let style = Style::default().fg(if owned { band_color } else { theme.muted });
    rendered.spans.insert(0, Span::styled(band, style));
    rendered
}

fn expand_tabs(text: &str, tab_width: usize) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch == '\t' {
            let pad = tab_width - (out.width() % tab_width.max(1));
            out.extend(std::iter::repeat_n(' ', pad));
        } else {
            out.push(ch);
        }
    }
    out
}

/// 概要。畳んだり途中で切ったりしない。これを読まずに項目から読み始めると、
/// 個々の変更が何のためかが分からないまま進むことになる。
fn overview(review: &Loaded, theme: &Theme, inner_w: usize) -> Vec<Line<'static>> {
    let overview = review.annotations.overview();
    let head = |text: String, color| {
        Line::styled(
            text,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )
    };

    let mut lines = vec![
        head("\u{2500}\u{2500} 概要".into(), theme.accent),
        Line::default(),
    ];
    push_since_previous(&mut lines, review, theme, inner_w);
    for (key, value) in [
        ("困っていたこと", &overview.problem),
        ("やったこと", &overview.change),
        ("仕組み", &overview.mechanism),
        ("置き場所", &overview.placement),
        ("範囲", &overview.scope),
    ] {
        lines.push(head(format!("  {key}"), theme.diff_section_header));
        for chunk in wrap(value, inner_w.saturating_sub(4)) {
            lines.push(Line::styled(
                format!("    {chunk}"),
                Style::default().fg(theme.fg),
            ));
        }
        lines.push(Line::default());
    }

    let impacts = review.annotations.impacts();
    if impacts.is_empty() {
        return lines;
    }
    lines.push(head("  機能への影響".into(), theme.warning));
    for impact in impacts {
        // 事実と推測を分けて出す。推測を事実の顔で出されると、確かめずに信じてしまう。
        let (tag, tag_color) = match impact.confidence {
            revidere::Confidence::Fact => ("事実", theme.diff_add),
            revidere::Confidence::Guess => ("推測", theme.diff_section_header),
        };
        lines.push(Line::from(vec![
            Span::styled(format!("    [{tag}] "), Style::default().fg(tag_color)),
            Span::styled(
                impact.feature.clone(),
                Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
            ),
        ]));
        for (label, value, color) in [
            ("変化", Some(&impact.change), theme.fg),
            ("確かめる", Some(&impact.verify), theme.muted),
            ("残る穴", impact.gap.as_ref(), theme.warning),
        ] {
            let Some(value) = value else { continue };
            let pad = " ".repeat(IMPACT_LABEL_W.saturating_sub(label.width()));
            let indent = " ".repeat(IMPACT_INDENT);
            push_hanging(
                &mut lines,
                &format!("{indent}{label}{pad} "),
                value,
                color,
                inner_w,
            );
        }
        lines.push(Line::default());
    }
    lines
}

/// 概要の先頭に置く。2 度目以降の読者が最初に知りたいのは「前と何が違うか」で、
/// 本体を読み直すかどうかもそれで決まる。初回は何も出さない。
fn push_since_previous(
    lines: &mut Vec<Line<'static>>,
    review: &Loaded,
    theme: &Theme,
    inner_w: usize,
) {
    let Some(since) = review.annotations.since_previous() else {
        return;
    };
    // 本文でも警告でもない補足はここに寄せる。muted は背景に埋もれるテーマがあり、
    // この節は 1 行しか出ないことがあるので、消えると節ごと壊れて見える。
    let note = theme.diff_section_header;
    lines.push(Line::styled(
        "  前回のレビューから".to_string(),
        Style::default().fg(note).add_modifier(Modifier::BOLD),
    ));
    lines.push(Line::styled(
        format!("    {} \u{2192} {}", since.previous_head, since.head),
        Style::default().fg(theme.fg),
    ));
    // 履歴が変わっていたら先に言う。前回のコミットが辿れない以上、下のファイル
    // 一覧は「積み上げ」ではなく「別の履歴との比較」になっている。
    if since.history_rewritten {
        push_note(
            lines,
            "前回のコミットは今の履歴から辿れない (rebase / amend / force push)。\
             下の一覧は前回との積み上げではなく、別々の履歴どうしの比較になる。",
            theme.warning,
            inner_w,
        );
    }
    match &since.files {
        // 引けなかったことを「無い」に畳まない。
        None => push_note(
            lines,
            "変わったファイルは一覧にできない (前回のコミットがもう残っていない)。",
            theme.warning,
            inner_w,
        ),
        Some(files) if files.is_empty() => {
            push_note(lines, "変わったファイルは無い", note, inner_w)
        }
        Some(files) => {
            for path in files.iter().take(SINCE_PREVIOUS_FILES_MAX) {
                lines.push(Line::styled(
                    format!("    {path}"),
                    Style::default().fg(theme.fg),
                ));
            }
            let rest = files.len().saturating_sub(SINCE_PREVIOUS_FILES_MAX);
            if rest > 0 {
                push_note(lines, &format!("ほか {rest} 件"), note, inner_w);
            }
            // ファイル名だけでは、指摘をどう直したのかは読めない。その行き先を指す。
            push_note(
                lines,
                "p: この区間だけのレビューへ (どこがどう変わったかを読む)",
                note,
                inner_w,
            );
        }
    }
    lines.push(Line::default());
}

fn push_note(lines: &mut Vec<Line<'static>>, text: &str, color: Color, inner_w: usize) {
    for chunk in wrap(text, inner_w.saturating_sub(4)) {
        lines.push(Line::styled(
            format!("    {chunk}"),
            Style::default().fg(color),
        ));
    }
}

fn push_hanging(
    lines: &mut Vec<Line<'static>>,
    head: &str,
    value: &str,
    color: Color,
    inner_w: usize,
) {
    let indent = head.width();
    for (n, chunk) in wrap(value, inner_w.saturating_sub(indent + 1))
        .into_iter()
        .enumerate()
    {
        let prefix = if n == 0 {
            head.to_string()
        } else {
            " ".repeat(indent)
        };
        lines.push(Line::styled(
            format!("{prefix}{chunk}"),
            Style::default().fg(color),
        ));
    }
}

/// 切るのは文字数ではなく表示幅。日本語は 1 文字 2 列なので、文字数で切ると幅の
/// 2 倍に伸びて、はみ出した分が枠で黙って落ちる。
fn wrap(text: &str, width: usize) -> Vec<String> {
    use unicode_width::UnicodeWidthChar;

    if width == 0 {
        return vec![text.to_string()];
    }
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        let mut used = 0;
        for ch in para.chars() {
            let w = UnicodeWidthChar::width(ch).unwrap_or(0);
            // 1 文字も入らない行は作らない (幅 1 に全角が来ても進む)。
            if used + w > width && !line.is_empty() {
                out.push(std::mem::take(&mut line));
                used = 0;
            }
            line.push(ch);
            used += w;
        }
        out.push(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 折り返しは表示幅で割り文字を1つも落とさない() {
        let got = wrap("あいうえおかきくけこ", 4);
        assert_eq!(got, ["あい", "うえ", "おか", "きく", "けこ"]);
        assert_eq!(got.concat().chars().count(), 10);
    }

    #[test]
    fn 混在テキストでも幅を超えない() {
        for line in wrap("abcあいdef うえおgh", 7) {
            assert!(line.width() <= 7, "{line:?} is wider than 7");
        }
    }

    /// 幅 0 で無限ループや空返しにならないこと。狭い端末で項目一覧の幅が潰れると
    /// ここへ来る。
    #[test]
    fn 幅0なら本文をそのまま返す() {
        assert_eq!(wrap("abc", 0), ["abc"]);
    }

    #[test]
    fn 段落の間の空行は残す() {
        assert_eq!(wrap("a\n\nb", 8), ["a", "", "b"]);
    }

    #[test]
    fn タブは表示幅で埋める() {
        assert_eq!(expand_tabs("\tx", 4), "    x");
        assert_eq!(expand_tabs("ab\tx", 4), "ab  x");
    }
}
