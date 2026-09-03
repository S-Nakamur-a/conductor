//! 表示幅を考慮した span の折り返し。

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// 表示上の1セル: スタイルを持つ1つの書記素クラスタ（grapheme cluster）。
/// 折り返しのヘルパーはこの粒度で動くので、改行をまたいでもスタイルが保たれる。
///
/// char ではなくクラスタにしているのは、両者の幅の数え方が両方向にずれるため:
/// ⚠ は1桁だが ⚠️（同じ文字に U+FE0F が続いたもの）は2桁になるので、char ごとに
/// 足すと過小に数えてしまい行がはみ出す。家族の絵文字のような ZWJ シーケンスは
/// 合計2桁なのに char としては7個あるので、char ごとに足すと過大に数えてしまい
/// 行が早く折り返される。クラスタなら改行で分割されることもないが、char 粒度では
/// それが平気で起きていた。
#[derive(Clone)]
pub(crate) struct Cell {
    pub(crate) text: String,
    pub(crate) style: Style,
}

impl Cell {
    pub(crate) fn new(ch: char, style: Style) -> Self {
        Self {
            text: ch.to_string(),
            style,
        }
    }

    pub(crate) fn width(&self) -> usize {
        UnicodeWidthStr::width(self.text.as_str())
    }

    pub(crate) fn is_space(&self) -> bool {
        self.text == " "
    }
}

pub(crate) fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

pub(crate) fn spans_to_cells(spans: &[Span<'static>]) -> Vec<Cell> {
    spans
        .iter()
        .flat_map(|s| {
            s.content.graphemes(true).map(move |g| Cell {
                text: g.to_string(),
                style: s.style,
            })
        })
        .collect()
}

pub(crate) fn cells_to_line(cells: &[Cell]) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let mut cur: Option<Style> = None;
    for cell in cells {
        match cur {
            Some(s) if s == cell.style => buf.push_str(&cell.text),
            _ => {
                if let Some(s) = cur {
                    spans.push(Span::styled(std::mem::take(&mut buf), s));
                }
                buf.push_str(&cell.text);
                cur = Some(cell.style);
            }
        }
    }
    if let Some(s) = cur {
        spans.push(Span::styled(buf, s));
    }
    if spans.is_empty() {
        Line::from("")
    } else {
        Line::from(spans)
    }
}

/// hard (コードブロック) はどのセル境界でも折り返す。そうでなければ単語境界を
/// 優先し、1 単語が長すぎるときだけハード分割する。
pub(crate) fn wrap_cells(cells: &[Cell], width: usize, hard: bool) -> Vec<Line<'static>> {
    wrap_cells_raw(cells, width, hard)
        .iter()
        .map(|l| cells_to_line(l))
        .collect()
}

/// 折り返し後もセル粒度のまま扱いたい呼び出し側が使う: テーブルの列は、横に並べる
/// 前に各行を列幅へパディング・アライメントする必要があり、セルが Span に潰れた
/// 後では不可能。
pub(crate) fn wrap_cells_raw(cells: &[Cell], width: usize, hard: bool) -> Vec<Vec<Cell>> {
    let width = width.max(1);
    let mut lines: Vec<Vec<Cell>> = Vec::new();
    let mut cur: Vec<Cell> = Vec::new();
    let mut cur_w = 0usize;

    let mut push_line = |cur: &mut Vec<Cell>, cur_w: &mut usize| {
        lines.push(std::mem::take(cur));
        *cur_w = 0;
    };

    if hard {
        for cell in cells {
            let cw = cell.width();
            if cur_w + cw > width && !cur.is_empty() {
                push_line(&mut cur, &mut cur_w);
            }
            cur.push(cell.clone());
            cur_w += cw;
        }
        if !cur.is_empty() {
            push_line(&mut cur, &mut cur_w);
        }
        if lines.is_empty() {
            lines.push(Vec::new());
        }
        return lines;
    }

    let n = cells.len();
    let mut i = 0;
    while i < n {
        if cells[i].is_space() {
            // スペース: 現在の（空でない）行に収まるときだけ残す。収まらない
            // 場合はここを折り返し位置として落とし、次の行に先頭スペースが
            // 残らないようにする。
            if !cur.is_empty() {
                if cur_w < width {
                    cur.push(cells[i].clone());
                    cur_w += 1;
                } else {
                    push_line(&mut cur, &mut cur_w);
                }
            }
            i += 1;
            continue;
        }

        // 次の単語（非スペースのセルの連続）をまとめる。
        let start = i;
        let mut word_w = 0;
        while i < n && !cells[i].is_space() {
            word_w += cells[i].width();
            i += 1;
        }
        let word = &cells[start..i];

        if word_w > width {
            // 1行より長い単語: 一旦フラッシュしてからハード分割する。
            if !cur.is_empty() {
                push_line(&mut cur, &mut cur_w);
            }
            for cell in word {
                let cw = cell.width();
                if cur_w + cw > width && !cur.is_empty() {
                    push_line(&mut cur, &mut cur_w);
                }
                cur.push(cell.clone());
                cur_w += cw;
            }
        } else {
            if cur_w + word_w > width && !cur.is_empty() {
                push_line(&mut cur, &mut cur_w);
            }
            for cell in word {
                cur.push(cell.clone());
                cur_w += cell.width();
            }
        }
    }
    if !cur.is_empty() {
        push_line(&mut cur, &mut cur_w);
    }
    if lines.is_empty() {
        lines.push(Vec::new());
    }
    lines
}

pub(crate) fn with_prefix(
    lines: Vec<Line<'static>>,
    first: Span<'static>,
    cont: Span<'static>,
) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .enumerate()
        .map(|(idx, line)| {
            let mut spans = Vec::with_capacity(line.spans.len() + 1);
            spans.push(if idx == 0 {
                first.clone()
            } else {
                cont.clone()
            });
            spans.extend(line.spans);
            Line::from(spans)
        })
        .collect()
}
