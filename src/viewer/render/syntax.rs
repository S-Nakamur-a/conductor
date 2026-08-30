//! ViewerState にキャッシュされた syntect のデータを使うシンタックスハイライトと
//! diff 注釈のヘルパー: diff 注釈キャッシュの構築、word-diff の span レンダラー、
//! シンタックストークンから Span への変換。

use crate::diff_state::InlineSegment;
use ratatui::style::{Color, Style};
use ratatui::text::Span;

/// 行内の diff セグメントを強調ハイライト付きで描画する。
/// シンタックストークンが使えない Delete 行向け。fg はプレーンテキストの色
/// （アクティブなテーマの前景色）。
pub(super) fn render_inline_diff_spans(
    segments: &[InlineSegment],
    diff_bg: Color,
    emphasis_bg: Color,
    fg: Color,
    tab_width: usize,
) -> Vec<Span<'static>> {
    segments
        .iter()
        .map(|seg| {
            let bg = if seg.emphasized { emphasis_bg } else { diff_bg };
            let text = expand_tabs(
                seg.text.trim_end_matches('\n').trim_end_matches('\r'),
                tab_width,
            );
            Span::styled(text, Style::default().fg(fg).bg(bg))
        })
        .collect()
}

/// シンタックスハイライトの前景色と word-diff の背景色をマージする。展開後の
/// セグメントテキストがシンタックストークンのテキストと一致しない場合は None を
/// 返す（呼び出し側はプレーン描画にフォールバックできる）。
pub(super) fn merge_syntax_with_inline(
    segments: &[InlineSegment],
    syntax_tokens: &[(Style, String)],
    diff_bg: Color,
    emphasis_bg: Color,
    tab_width: usize,
) -> Option<Vec<Span<'static>>> {
    // インラインセグメントから展開後のテキストと、バイトごとの強調フラグを
    // 構築する。タブはセグメントをまたいだ共有の列カウンタで展開するので、
    // 結果が下のシンタックストークンの列一致した展開と揃う。
    let mut expanded_text = String::new();
    let mut byte_emphasis: Vec<bool> = Vec::new();

    let mut col = 0;
    for seg in segments {
        let trimmed = seg.text.trim_end_matches('\n').trim_end_matches('\r');
        let expanded = expand_tabs_at(trimmed, tab_width, &mut col);
        byte_emphasis.resize(byte_emphasis.len() + expanded.len(), seg.emphasized);
        expanded_text.push_str(&expanded);
    }

    // シンタックストークンからバイトごとの前景スタイルを構築する。シンタックス
    // キャッシュは生の（未展開の）タブを保持しているので、ここでも同じ共有の
    // 列カウンタで展開する — さもないとタブを含む行は下の一致判定に必ず失敗し、
    // シンタックス＋強調の色付けが黙って失われてしまう。
    let mut syntax_text = String::new();
    let mut byte_fg: Vec<Style> = Vec::new();

    let mut col = 0;
    for (style, text) in syntax_tokens {
        let trimmed = text.trim_end_matches('\n').trim_end_matches('\r');
        let expanded = expand_tabs_at(trimmed, tab_width, &mut col);
        byte_fg.resize(byte_fg.len() + expanded.len(), *style);
        syntax_text.push_str(&expanded);
    }

    // タブ展開後にテキストが一致していなければならない。一致しなければ諦める。
    if expanded_text != syntax_text {
        return None;
    }

    let len = expanded_text.len();
    let mut result: Vec<Span<'static>> = Vec::new();
    let mut i = 0;

    while i < len {
        let start = i;
        let emph = byte_emphasis[i];
        let fg = byte_fg[i];
        let bg = if emph { emphasis_bg } else { diff_bg };

        i += 1;
        while i < len {
            let next_emph = byte_emphasis[i];
            let next_fg_color = byte_fg[i].fg;
            if next_emph != emph || next_fg_color != fg.fg {
                break;
            }
            i += 1;
        }

        // UTF-8 の文字境界に必ず着地させる。
        while i < len && !expanded_text.is_char_boundary(i) {
            i += 1;
        }

        result.push(Span::styled(expanded_text[start..i].to_string(), fg.bg(bg)));
    }

    Some(result)
}

/// タブ文字をスペースに展開する。ビューアのタブ展開と同じ挙動。
fn expand_tabs(line: &str, tab_width: usize) -> String {
    if !line.contains('\t') {
        return line.to_string();
    }
    let mut col = 0;
    expand_tabs_at(line, tab_width, &mut col)
}

/// 列 col から始めてタブを展開し、col をその断片ぶん進める。
///
/// 1行の連続する断片間で共有の col を引き継ぐことで、タブ位置が常に列として
/// 正しく保たれる。これにより、同じ行に対する2種類のトークン化
/// （word-diff のセグメントとシンタックストークン）が同一のテキストに展開される。
pub(crate) fn expand_tabs_at(piece: &str, tab_width: usize, col: &mut usize) -> String {
    let mut result = String::with_capacity(piece.len());
    for ch in piece.chars() {
        if ch == '\t' {
            let spaces = tab_width - (*col % tab_width);
            for _ in 0..spaces {
                result.push(' ');
            }
            *col += spaces;
        } else {
            result.push(ch);
            *col += 1;
        }
    }
    result
}

/// syntect のハイライトキャッシュから、1行ぶんの ratatui Span を返す。
///
/// diff_bg が指定されていれば、トークンの前景色は保ったまま背景色を diff の色で
/// 上書きする。その行のキャッシュエントリが無ければ、プレーンな白色にフォール
/// バックする。
pub(super) fn syntax_spans_for_line(
    vs: &crate::viewer::ViewerState,
    line_no: usize,
    diff_bg: Option<Color>,
    fg: Color,
) -> Vec<Span<'static>> {
    if let Some(tokens) = vs.content.highlighted_lines.get(line_no) {
        tokens
            .iter()
            .map(|(style, text)| {
                let s = if let Some(bg) = diff_bg {
                    // トークンの前景色は保ち、背景色を diff の色で上書きする。
                    style.bg(bg)
                } else {
                    *style
                };
                Span::styled(text.clone(), s)
            })
            .collect()
    } else {
        // フォールバック: テーマの前景色でプレーンテキストを表示する。
        let text = vs
            .content
            .file_content
            .get(line_no)
            .cloned()
            .unwrap_or_default();
        vec![Span::styled(text, Style::default().fg(fg))]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(text: &str, emphasized: bool) -> InlineSegment {
        InlineSegment {
            text: text.to_string(),
            emphasized,
        }
    }

    #[test]
    fn merge_handles_tabbed_lines() {
        // 生のタブを含む2つのシンタックストークンとしてハイライトされた
        // "\tlet x" という行。word-diff のセグメントはタブを展開するので、
        // シンタックストークン側も同じ方法で展開しなければ、マージは黙って
        // プレーン描画にフォールバックしてしまう。
        let segments = vec![seg("\tlet ", false), seg("x", true)];
        let syntax_tokens = vec![
            (Style::default().fg(Color::Red), "\t".to_string()),
            (Style::default().fg(Color::Blue), "let x".to_string()),
        ];
        let merged = merge_syntax_with_inline(
            &segments,
            &syntax_tokens,
            Color::Rgb(0, 40, 0),
            Color::Rgb(0, 80, 0),
            4,
        );
        // このタブ修正の前は None が返っていた（タブの部分でテキストが不一致になっていた）。
        let spans = merged.expect("tabbed line should merge, not fall back to plain");
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, "    let x"); // タブが列0で4スペースに展開される
    }

    #[test]
    fn merge_bails_on_text_mismatch() {
        // タブだけでなく本当にテキストが異なる場合も、呼び出し側がプレーン描画に
        // フォールバックできるよう諦めなければならない。
        let segments = vec![seg("foo", false)];
        let syntax_tokens = vec![(Style::default(), "bar".to_string())];
        let merged = merge_syntax_with_inline(
            &segments,
            &syntax_tokens,
            Color::Rgb(0, 40, 0),
            Color::Rgb(0, 80, 0),
            4,
        );
        assert!(merged.is_none());
    }
}
