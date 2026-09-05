//! tool_use / tool_result の行。⏺ と ⎿ のレイアウトは claude_log の分類が決める。

use std::collections::HashMap;

use conductor_core::claude_log::{
    BUCKET_ORDER, CountedBucket, DisplayBlock, LogEntry, ResultKind, ToolCategory, classify,
    unknown_tool_arg,
};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::style::{
    ASSISTANT_MARKER, MARKER_COLS, TOOL_RESULT_GLYPH, ToolStyles, fit_styled_line, pad_glyph_to,
    truncate_to_width,
};
use super::wrap::wrap_plain_text;

const EXPAND_HINT: &str = " (ctrl+o to expand)";
/// "  ⎿  " の表示カラム数。継続行もここまで字下げする。
const RESULT_GUTTER: usize = 5;

/// エントリ内の Counted な結果をバケットごとに数える。
///
/// シェル経由はフォールバックとしてしか数えない。実測: cat×3 + Read×1 は
/// "Read 1 file" になり、本来のツールが 1 件でも寄与すると近似値は捨てられる。
/// Counted は is_error を無視する (失敗した Read も普通のサマリに畳まれる)。
pub(super) fn count_buckets(entry: &LogEntry) -> HashMap<CountedBucket, usize> {
    let mut native: HashMap<CountedBucket, usize> = HashMap::new();
    let mut shell: HashMap<CountedBucket, usize> = HashMap::new();
    for block in &entry.blocks {
        if let DisplayBlock::ToolResult {
            kind: ResultKind::Counted { bucket, from_bash },
            ..
        } = block
        {
            let target = if *from_bash { &mut shell } else { &mut native };
            *target.entry(*bucket).or_insert(0) += 1;
        }
    }
    BUCKET_ORDER
        .into_iter()
        .filter_map(|bucket| {
            let n = match native.get(&bucket).copied().unwrap_or(0) {
                0 => shell.get(&bucket).copied().unwrap_or(0),
                n => n,
            };
            (n > 0).then_some((bucket, n))
        })
        .collect()
}

/// 折りたたみ表示の Counted / Hidden は tool_result 側で描くか描かないので None。
///
/// errored でマーカーを赤にするのは、折りたたみの Inline で実測した挙動を
/// 全カテゴリと展開表示にも広げたもの。反証は見つかっていない。
pub(super) fn render_tool_use(
    name: &str,
    input: &serde_json::Value,
    errored: bool,
    expanded: bool,
    width: usize,
    styles: &ToolStyles,
) -> Option<Line<'static>> {
    let (display_name, arg) = if expanded {
        (name.to_string(), unknown_tool_arg(input))
    } else {
        match classify(name, input) {
            ToolCategory::Counted(_) | ToolCategory::Hidden => return None,
            ToolCategory::Inline { display_name, arg } => (display_name, arg),
        }
    };
    let marker_style = if errored {
        styles.marker_err
    } else {
        styles.marker
    };
    Some(tool_use_line(
        &display_name,
        arg.as_deref(),
        width,
        marker_style,
        styles,
    ))
}

/// ⏺ {name}({arg})。名前を描いた後に余地が残らなければ括弧ごと落とす — 狭い区画で
/// 長い MCP ツール名を無条件に出すとパネルからはみ出していた。
fn tool_use_line(
    display_name: &str,
    arg: Option<&str>,
    width: usize,
    marker_style: Style,
    styles: &ToolStyles,
) -> Line<'static> {
    let marker = pad_glyph_to(ASSISTANT_MARKER, MARKER_COLS);
    let remaining = width.saturating_sub(MARKER_COLS);
    let name_cols = UnicodeWidthStr::width(display_name);

    let arg = arg
        .filter(|s| !s.is_empty())
        .and_then(|a| {
            let budget = remaining.checked_sub(name_cols + 2).filter(|b| *b > 0)?;
            Some(truncate_to_width(a, budget))
        })
        .filter(|s| !s.is_empty());

    match arg {
        None => Line::from(vec![
            Span::styled(marker, marker_style),
            Span::styled(truncate_to_width(display_name, remaining), styles.name),
        ]),
        Some(arg) => Line::from(vec![
            Span::styled(marker, marker_style),
            Span::styled(display_name.to_string(), styles.name),
            Span::styled(format!("({arg})"), styles.arg),
        ]),
    }
}

/// 折りたたみ表示の tool_result。
///
/// Hidden は失敗しても何も描かない (is_error を持つ TodoWrite の出力は 1 行も無かった)。
/// Counted はエントリ内の全件が 1 行を共有するので、最初の 1 件だけが描く。
pub(super) fn render_result_collapsed(
    kind: ResultKind,
    lines: &[String],
    is_error: bool,
    counts: &HashMap<CountedBucket, usize>,
    summary_emitted: &mut bool,
    width: usize,
    styles: &ToolStyles,
) -> Vec<Line<'static>> {
    match kind {
        ResultKind::Hidden => Vec::new(),
        ResultKind::Inline if is_error => inline_error_lines(lines, width, styles),
        ResultKind::Inline => Vec::new(),
        ResultKind::Counted { .. } if std::mem::replace(summary_emitted, true) => Vec::new(),
        ResultKind::Counted { .. } => vec![bucket_summary_line(counts, width, styles)],
    }
}

/// 節は BUCKET_ORDER の順、先頭の動詞だけ大文字のまま、件数は太字。すべて実測。
fn bucket_summary_line(
    counts: &HashMap<CountedBucket, usize>,
    width: usize,
    styles: &ToolStyles,
) -> Line<'static> {
    let mut parts: Vec<(String, Style)> = Vec::new();
    for bucket in BUCKET_ORDER {
        let Some(&n) = counts.get(&bucket) else {
            continue;
        };
        let (verb, singular, plural) = bucket.labels();
        let lead = if parts.is_empty() {
            format!("{verb} ")
        } else {
            format!(", {} ", lower_first(verb))
        };
        parts.push((lead, styles.result));
        parts.push((n.to_string(), styles.result.add_modifier(Modifier::BOLD)));
        parts.push((
            format!(" {}", if n == 1 { singular } else { plural }),
            styles.result,
        ));
    }
    parts.push((EXPAND_HINT.to_string(), styles.result));
    fit_styled_line(MARKER_COLS, &parts, width)
}

fn lower_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_lowercase().collect::<String>() + chars.as_str(),
    }
}

/// 失敗した Bash(false) からカラム単位で実測。1 行目は col0 に灰色のスペース、
/// col2 に ⎿、本文は col5 から。継続行は接頭辞なしで col5 から。
fn inline_error_lines(lines: &[String], width: usize, styles: &ToolStyles) -> Vec<Line<'static>> {
    let budget = width.saturating_sub(RESULT_GUTTER);
    let first = lines.first().map(String::as_str).unwrap_or("(no content)");
    let mut out = vec![Line::from(vec![
        Span::styled(" ".to_string(), styles.result),
        Span::styled(format!(" {TOOL_RESULT_GLYPH}  "), styles.result_err),
        Span::styled(
            truncate_to_width(&format!("Error: {first}"), budget),
            styles.result_err,
        ),
    ])];
    out.extend(lines.iter().skip(1).map(|raw| {
        Line::from(vec![
            Span::raw(" ".repeat(RESULT_GUTTER)),
            Span::styled(truncate_to_width(raw, budget), styles.result_err),
        ])
    }));
    out
}

/// CLI が上のブロックに付ける ⎿ 行。tool_result と同じ溝だが、切り詰めずに折り返す。
/// 実測: worktree の外を指す ../../../… パスは省略されずに 2 行目へ続く。
pub(super) fn render_annotation(
    lines: &[String],
    width: usize,
    styles: &ToolStyles,
) -> Vec<Line<'static>> {
    let prefix = format!("  {TOOL_RESULT_GLYPH}  ");
    let indent = " ".repeat(RESULT_GUTTER);
    let budget = width.saturating_sub(RESULT_GUTTER);

    let mut out: Vec<Line<'static>> = Vec::new();
    for raw in lines {
        for wrapped in wrap_plain_text(raw, budget) {
            let gutter = if out.is_empty() {
                Span::styled(prefix.clone(), styles.result)
            } else {
                Span::raw(indent.clone())
            };
            out.push(Line::from(vec![
                gutter,
                Span::styled(wrapped, styles.result),
            ]));
        }
    }
    out
}

/// 展開表示の tool_result。折りたたみと同じレイアウトのまま、行数の上限だけ外す。
pub(super) fn render_result_expanded(
    lines: &[String],
    is_error: bool,
    width: usize,
    styles: &ToolStyles,
) -> Vec<Line<'static>> {
    let prefix = format!("  {TOOL_RESULT_GLYPH}  ");
    let indent = " ".repeat(RESULT_GUTTER);
    let connector = if is_error {
        styles.result_err
    } else {
        styles.result
    };

    if lines.is_empty() {
        let s = truncate_to_width(&format!("{prefix}(no content)"), width);
        return vec![Line::from(Span::styled(s, connector))];
    }

    lines
        .iter()
        .enumerate()
        .map(|(i, raw)| {
            let body = truncate_to_width(raw, width.saturating_sub(RESULT_GUTTER));
            let gutter = if i == 0 {
                Span::styled(prefix.clone(), connector)
            } else {
                Span::raw(indent.clone())
            };
            Line::from(vec![gutter, Span::styled(body, styles.result)])
        })
        .collect()
}
