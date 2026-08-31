//! ファイルビューと diff ビューで共有する汎用の Span/Line 操作ヘルパー: 水平スクロール
//! によるクリップ、下線・ヒントラベルのオーバーレイ、ガター用の桁数計算。

use crate::theme::Theme;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

/// Span の並びの先頭から offset 文字ぶんスキップし、最大 max_width 文字に
/// 切り詰める。span ごとのスタイルは保ったまま行う。
pub(super) fn h_scroll_spans(
    spans: Vec<Span<'static>>,
    offset: usize,
    max_width: usize,
) -> Vec<Span<'static>> {
    let mut remaining_skip = offset;
    let mut remaining_width = max_width;
    let mut result: Vec<Span<'static>> = Vec::new();
    for span in spans {
        if remaining_width == 0 {
            break;
        }
        let char_count = span.content.chars().count();
        // 左側のクリッピング: 水平スクロールのオフセットぶん文字をスキップする。
        if remaining_skip > 0 {
            if remaining_skip >= char_count {
                remaining_skip -= char_count;
                continue;
            }
            let s: String = span.content.chars().skip(remaining_skip).collect();
            let len = s.chars().count();
            if len <= remaining_width {
                remaining_width -= len;
                result.push(Span::styled(s, span.style));
            } else {
                let truncated: String = s.chars().take(remaining_width).collect();
                remaining_width = 0;
                result.push(Span::styled(truncated, span.style));
            }
            remaining_skip = 0;
        } else {
            // 右側のクリッピング: 残りのパネル幅に切り詰める。
            if char_count <= remaining_width {
                remaining_width -= char_count;
                result.push(span);
            } else {
                let truncated: String = span.content.chars().take(remaining_width).collect();
                remaining_width = 0;
                result.push(Span::styled(truncated, span.style));
            }
        }
    }
    result
}

pub(super) fn digit_count(n: usize) -> usize {
    if n == 0 {
        return 1;
    }
    let mut count = 0;
    let mut val = n;
    while val > 0 {
        count += 1;
        val /= 10;
    }
    count
}

/// 元のコンテンツの start_col..end_col の範囲にある span に、下線とアクセント色の
/// 前景色を適用する。h_scroll は span にすでに適用済みの水平スクロールオフセット。
pub(super) fn apply_underline_range(
    spans: Vec<Span<'static>>,
    start_col: usize,
    end_col: usize,
    h_scroll: usize,
    accent: Color,
) -> Vec<Span<'static>> {
    // 元のコンテンツの列を、h_scroll 適用後の表示列に変換する。
    let vis_start = start_col.saturating_sub(h_scroll);
    let vis_end = end_col.saturating_sub(h_scroll);
    if vis_start >= vis_end {
        return spans;
    }

    let mut result = Vec::with_capacity(spans.len() + 4);
    let mut pos: usize = 0;
    for span in spans {
        let span_len = span.content.chars().count();
        let span_end = pos + span_len;

        if span_end <= vis_start || pos >= vis_end {
            // 下線の範囲の外側にある。
            result.push(span);
        } else {
            // この span は下線の範囲と重なっている。
            let rel_start = vis_start.saturating_sub(pos);
            let rel_end = vis_end.saturating_sub(pos).min(span_len);

            let chars: Vec<char> = span.content.chars().collect();

            // 下線より前の部分。
            if rel_start > 0 {
                let before: String = chars[..rel_start].iter().collect();
                result.push(Span::styled(before, span.style));
            }
            // 下線を引く部分。
            let underlined: String = chars[rel_start..rel_end].iter().collect();
            result.push(Span::styled(
                underlined,
                span.style.fg(accent).add_modifier(Modifier::UNDERLINED),
            ));
            // 下線より後の部分。
            if rel_end < span_len {
                let after: String = chars[rel_end..].iter().collect();
                result.push(Span::styled(after, span.style));
            }
        }
        pos = span_end;
    }
    result
}

/// span に Vimium 風のヒントラベルを適用し、ヒント対象シンボルの先頭2文字を
/// ラベル文字（アクセント色＋太字）に置き換える。
pub(super) fn apply_hint_labels(
    spans: Vec<Span<'static>>,
    hints: &[&crate::overlay::SymbolHint],
    input: &str,
    h_scroll: usize,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let mut result = spans;
    // ヒントを逆順に処理することで、先に行った置き換えが後のヒントの位置を
    // ずらさないようにする。
    let mut sorted: Vec<&&crate::overlay::SymbolHint> = hints.iter().collect();
    sorted.sort_by_key(|h| std::cmp::Reverse(h.start_col));

    for hint in sorted {
        let vis_start = hint.start_col.saturating_sub(h_scroll);
        let label_len = hint.label.chars().count();
        let vis_end = vis_start + label_len;

        // このヒントが現在の入力に一致するかを判定する。
        let is_matching = input.is_empty() || hint.label.starts_with(input);
        let label_style = if is_matching {
            Style::default()
                .fg(theme.selected_fg)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.muted).bg(Color::Reset)
        };

        // vis_start..vis_end の文字をラベルに置き換える。
        result = replace_span_range(result, vis_start, vis_end, &hint.label, label_style);
    }
    result
}

/// span のリストのうち [start..end) の範囲の文字を、指定スタイルの置換テキストで
/// 置き換える。
fn replace_span_range(
    spans: Vec<Span<'static>>,
    start: usize,
    end: usize,
    replacement: &str,
    style: Style,
) -> Vec<Span<'static>> {
    let mut result = Vec::with_capacity(spans.len() + 4);
    let mut pos: usize = 0;

    for span in spans {
        let span_len = span.content.chars().count();
        let span_end = pos + span_len;

        if span_end <= start || pos >= end {
            // 置換範囲の外側にある。
            result.push(span);
        } else {
            let chars: Vec<char> = span.content.chars().collect();
            let rel_start = start.saturating_sub(pos);
            let rel_end = end.saturating_sub(pos).min(span_len);

            // 置換より前の部分。
            if rel_start > 0 {
                let before: String = chars[..rel_start].iter().collect();
                result.push(Span::styled(before, span.style));
            }
            // 置換部分（最初に重なった span からのみ1回出力する）。
            if pos <= start {
                result.push(Span::styled(replacement.to_string(), style));
            }
            // 置換より後の部分。
            if rel_end < span_len {
                let after: String = chars[rel_end..].iter().collect();
                result.push(Span::styled(after, span.style));
            }
        }
        pos = span_end;
    }
    result
}
