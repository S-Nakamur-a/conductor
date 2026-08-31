//! tool_use/tool_result 行の描画。⏺/⎿ のレイアウトは crate::claude_log のツール分類によって決まる。
//!
//! 分類主導のレイアウトルールは、エントリを走査して Markdown のテキスト/thinking ブロックを
//! 描画するのとは別の関心事なので、[build](super::build) から切り出してある。

use std::collections::HashMap;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::reflow::log::{
    BUCKET_ORDER, CountedBucket, DisplayBlock, LogEntry, ResultKind, ToolCategory, classify,
    unknown_tool_arg,
};

use super::glyphs::{ASSISTANT_MARKER, MARKER_COLS, TOOL_RESULT_GLYPH};
use super::helpers::{fit_styled_line, pad_glyph_to, truncate_to_width};

/// このモジュールの描画関数が使う固定スタイル群を1つの構造体にまとめてある。スタイルごとに
/// 引数を分けるのではなく、これ1つを渡す形にするため（clippy の too_many_arguments の上限は7で、
/// render_tool_result_collapsed だけで非スタイル引数を5個使うので、スタイル用に2個以上を
/// 別々に取る余地がない）。
pub(crate) struct ToolStyles {
    pub marker: Style,
    pub marker_err: Style,
    pub name: Style,
    pub arg: Style,
    pub result: Style,
    pub result_err: Style,
}

/// エントリごとに、tool_result ブロックがどの [CountedBucket] へ何個振り分けられるかを
/// 事前に数える。[render_tool_result_collapsed] がこれを使い、1件ずつではなくバケットの
/// 初出時に集約した「{verb} N {noun}」の1行だけを描画する。
///
/// Counted は is_error を完全に無視する（実測: 失敗した Read でもエラー表示なしの
/// 「Read 1 file」というプレーンなサマリに畳み込まれる）ため、バケットを持つ結果は
/// エラーフラグに関わらずすべてここでカウントする。
///
/// 集計範囲は連続した一致結果ではなくエントリ全体である。Claude は1つの assistant ターンの
/// 全結果を後続の1つの user エントリにまとめてバッチ送信するので、エントリ単位のカウントが
/// 実際に観測される形と一致し、「途中で打ち切られたか」を別途追跡する必要もない。
pub(crate) fn count_buckets(entry: &LogEntry) -> HashMap<CountedBucket, usize> {
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
    // シェル経由の呼び出しはあくまでフォールバックとしてのみ数える。実測（各1箇所での
    // 呼び出しで確認）: cat×1 → "Read 1 file"、cat×2 → "Read 2 files"、しかし
    // cat×3 + Read×1 → "Read 1 file"。つまりバケット本来のツールが少しでも寄与すると、
    // シェル側の近似値は完全に捨てられる。（List にはネイティブツールが無いので常に
    // シェルカウントへフォールバックする。Search はシェル由来を持たない。）
    let mut counts = HashMap::new();
    for bucket in BUCKET_ORDER {
        let n = match native.get(&bucket).copied().unwrap_or(0) {
            0 => shell.get(&bucket).copied().unwrap_or(0),
            n => n,
        };
        if n > 0 {
            counts.insert(bucket, n);
        }
    }
    counts
}

/// tool_use ブロックを1つ描画する。何も描かない場合は None を返す（折りたたみモードでの
/// Counted/Hidden カテゴリ — これらは tool_result 側の位置で描くか、まったく描かない）。
///
/// 展開モードでは常に描画し、折りたたみモードでのエイリアス（Edit → Update など）ではなく
/// ツール自身の生の名前を使う。引数は [unknown_tool_arg] の汎用キー検索によるベストエフォート
/// （展開モードには折りたたみモードの Inline カテゴリのような、ツールごとの「これが引数」
/// という決まった鍵が無いため）。
///
/// errored はマーカー色を選ぶ（折りたたみモードの Inline について実測済み: 失敗した
/// Bash(false) はその ⏺ を緑ではなく palette::ERROR で描く — tool_class::ToolCategory::Inline
/// を参照）。これは全カテゴリと展開モードにも一律で適用している。反証となる実測例が無く、
/// 「この呼び出しが失敗したか」というシグナル自体はどちらのモードでも同じだからであり、
/// これ自体は実測ではなく自分で決めた一般化である。
pub(crate) fn render_tool_use(
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

/// ⏺ {display_name}({arg}) — 先頭の丸（色は marker_style で指定）、太字の名前、
/// 続けて自分のスタイルで引数（引数が無ければ括弧ごと省く）。
fn tool_use_line(
    display_name: &str,
    arg: Option<&str>,
    width: usize,
    marker_style: Style,
    styles: &ToolStyles,
) -> Line<'static> {
    let marker_prefix = pad_glyph_to(ASSISTANT_MARKER, MARKER_COLS);
    let remaining = width.saturating_sub(MARKER_COLS);
    let name_cols = UnicodeWidthStr::width(display_name);

    // 括弧付き引数の予算は、名前を描いた後に余地が残る場合だけ確保する。引数が
    // 何も残らないなら括弧ごと省く。以前は名前を予算チェックなしに出力していたため、
    // 狭いパネルで長い MCP ツール名だとはみ出していた — ⏺ mcp__ccgrep__search() は
    // 幅20だと23カラムある — その状態で予算が飽和すると素の Name() だけが描かれていた。
    let arg_display = arg
        .filter(|s| !s.is_empty())
        .and_then(|a| {
            let budget = remaining.checked_sub(name_cols + 2).filter(|b| *b > 0)?;
            Some(truncate_to_width(a, budget))
        })
        .filter(|s| !s.is_empty());

    match arg_display {
        None => Line::from(vec![
            Span::styled(marker_prefix, marker_style),
            Span::styled(truncate_to_width(display_name, remaining), styles.name),
        ]),
        Some(arg) => Line::from(vec![
            Span::styled(marker_prefix, marker_style),
            Span::styled(display_name.to_string(), styles.name),
            Span::styled(format!("({arg})"), styles.arg),
        ]),
    }
}

/// 折りたたみモードで tool_result ブロックを1つ描画する。何も描かない場合は空の Vec を
/// 返す（エラーでない Inline/Hidden の結果、またはこのエントリ内で既に集約済みの
/// Counted バケットの再出現）。
///
/// [ResultKind::Counted] は is_error を完全に無視する — これは推測ではなく実測で、
/// 失敗した Read でもエラー表示なしのプレーンな「Read 1 file (ctrl+o to expand)」
/// サマリに畳み込まれる。エントリ内のすべての Counted 結果は1行を共有する（複数の
/// バケットはカンマ区切りの節リストとして1行にまとめて描かれる）ので、summary_emitted
/// はエントリごとに1つだけのラッチになっている。最初の結果が [bucket_summary_line] を
/// 描き、以降は何も描かない。
///
/// [ResultKind::Inline] はエラー時のみ描画し、実測済みの複数行 ⎿ Error: … レイアウトを
/// 使う。[ResultKind::Hidden] はエラーの有無に関わらずまったく何も描かない — これも
/// 実測（is_error を持つ TodoWrite は出力が無かった）。
pub(crate) fn render_tool_result_collapsed(
    kind: ResultKind,
    lines: &[String],
    is_error: bool,
    counts: &HashMap<CountedBucket, usize>,
    summary_emitted: &mut bool,
    width: usize,
    styles: &ToolStyles,
) -> Vec<Line<'static>> {
    match kind {
        // 実測: is_error を持つ結果を返した TodoWrite でも出力は1行も無かった。
        // Hidden は失敗しても Hidden のままである。
        ResultKind::Hidden => Vec::new(),
        ResultKind::Inline => {
            if is_error {
                inline_error_lines(lines, width, styles)
            } else {
                Vec::new()
            }
        }
        ResultKind::Counted { .. } => {
            // エントリ内のすべての Counted 結果は1つの共有行に畳み込まれ、その最初の
            // ものが描画を担う。
            if std::mem::replace(summary_emitted, true) {
                Vec::new()
            } else {
                vec![bucket_summary_line(counts, width, styles)]
            }
        }
    }
}

/// s の先頭1文字だけを小文字化し、残りはそのままにする
/// （"Searched for" → "searched for"）。
fn lower_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_lowercase().collect::<String>() + chars.as_str(),
    }
}

const EXPAND_HINT: &str = " (ctrl+o to expand)";

/// 節の順序は [BUCKET_ORDER]。最初の節の動詞だけ大文字を保ち、以降は小文字化する。
/// 件数は太字。すべて実測に基づく。
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
        let noun = if n == 1 { singular } else { plural };
        let lead = if parts.is_empty() {
            format!("{verb} ")
        } else {
            format!(", {} ", lower_first(verb))
        };
        parts.push((lead, styles.result));
        parts.push((n.to_string(), styles.result.add_modifier(Modifier::BOLD)));
        parts.push((format!(" {noun}"), styles.result));
    }
    parts.push((EXPAND_HINT.to_string(), styles.result));
    fit_styled_line(MARKER_COLS, &parts, width)
}

/// 失敗した Bash(false) のキャプチャからカラム単位で実測。1 行目は col0 に灰色のスペース、
/// col2 に ⎿、本文 ("Error: " 付き) は col5 から。続きの行は col5 から接頭辞なし。
fn inline_error_lines(lines: &[String], width: usize, styles: &ToolStyles) -> Vec<Line<'static>> {
    let first_budget = width.saturating_sub(5);
    let cont_budget = width.saturating_sub(5);
    let cont_indent = " ".repeat(5);

    let first_raw = lines.first().map(String::as_str).unwrap_or("(no content)");
    let first_body = truncate_to_width(&format!("Error: {first_raw}"), first_budget);
    let mut out = vec![Line::from(vec![
        Span::styled(" ".to_string(), styles.result), // col0: 灰色のスペース1個
        Span::styled(format!(" {TOOL_RESULT_GLYPH}  "), styles.result_err), // cols1-4
        Span::styled(first_body, styles.result_err),  // col5以降
    ])];

    for raw in lines.iter().skip(1) {
        let body = truncate_to_width(raw, cont_budget);
        out.push(Line::from(vec![
            Span::raw(cont_indent.clone()),
            Span::styled(body, styles.result_err),
        ]));
    }
    out
}

/// [DisplayBlock::Annotation] を描画する — CLI が上のブロックに付ける ⎿ 行（スラッシュ
/// コマンドの標準出力、compact をまたいで持ち越されたファイルなど）。
///
/// tool_result と同じガター（  ⎿   = 5カラム、継続行は本文の下に揃える）だが、テキストは
/// 切り詰められるのではなく折り返される。実測: worktree の外にあるファイルは
/// どのパネルもはみ出すほど長い ../../../… パスになり、Claude Code はそれを省略せず
/// 2行目へ続ける —
/// ```text
///   ⎿  Read ../../../../private/tmp/claude-501/-Users-…-plan/82e28e51-5e62-421c-aa82-d6
///      b09226bf7b/scratchpad/try-release.sh (82 lines)
/// ```
/// — 折り返しがパスの途中で入っている点に注目。これは予算より広い単語に対して
/// wrap_plain_text が行う、機械的なカラム分割そのものである。
pub(crate) fn render_annotation(
    lines: &[String],
    width: usize,
    styles: &ToolStyles,
) -> Vec<Line<'static>> {
    let first_prefix = format!("  {TOOL_RESULT_GLYPH}  ");
    let prefix_cols = UnicodeWidthStr::width(first_prefix.as_str());
    let cont_indent = " ".repeat(prefix_cols);
    let budget = width.saturating_sub(prefix_cols);

    let mut out = Vec::new();
    for raw in lines {
        for wrapped in super::user_text::wrap_plain_text(raw, budget) {
            let prefix = if out.is_empty() {
                Span::styled(first_prefix.clone(), styles.result)
            } else {
                Span::raw(cont_indent.clone())
            };
            out.push(Line::from(vec![
                prefix,
                Span::styled(wrapped, styles.result),
            ]));
        }
    }
    out
}

/// 展開モードで tool_result ブロックを描画する: 出力の全行を、Claude Code の折りたたみ
/// ⎿ ブロックと同じレイアウトで、ただしプレビュー上限なしに並べる — すべて表示することこそ
/// 展開の目的である。
pub(crate) fn render_tool_result_expanded(
    lines: &[String],
    is_error: bool,
    width: usize,
    styles: &ToolStyles,
) -> Vec<Line<'static>> {
    let body_style = styles.result;
    // "  ⎿  " — 2スペースのインデント + 1カラムのグリフ + 2スペース = 5カラム。
    // 継続行も同じ幅だけインデントし、出力テキストの左端を揃える。
    let first_prefix = format!("  {TOOL_RESULT_GLYPH}  ");
    let prefix_cols = UnicodeWidthStr::width(first_prefix.as_str());
    let cont_indent = " ".repeat(prefix_cols);
    let connector_style = if is_error {
        styles.result_err
    } else {
        body_style
    };

    if lines.is_empty() {
        let s = truncate_to_width(&format!("{first_prefix}(no content)"), width);
        return vec![Line::from(Span::styled(s, connector_style))];
    }

    lines
        .iter()
        .enumerate()
        .map(|(i, raw)| {
            let body = truncate_to_width(raw, width.saturating_sub(prefix_cols));
            if i == 0 {
                Line::from(vec![
                    Span::styled(first_prefix.clone(), connector_style),
                    Span::styled(body, body_style),
                ])
            } else {
                Line::from(vec![
                    Span::raw(cont_indent.clone()),
                    Span::styled(body, body_style),
                ])
            }
        })
        .collect()
}
