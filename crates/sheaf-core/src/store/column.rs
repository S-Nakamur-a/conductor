//! occurrence の範囲を、実際に使ってよいバイト位置として読む。
//!
//! 索引が列をどう数えているかは producer ごとに違い、宣言されないこともある。
//! 数え方を決めきれない occurrence を答えから外すのはここの仕事。

use super::scip_split;
use crate::{Location, Span};
use std::path::Path;

/// occurrence の範囲が、聞かれた語の範囲に収まっているか。
///
/// 覆うかではなく収まるかで判定する。occurrence の範囲は語より広いことがあり
/// （`==` に付く範囲は前後の空白を含み、モジュール自身の定義はファイル全体を覆う）、
/// 語より広い範囲はその語についての主張ではない。
/// range は 3 要素と 4 要素の両方が実在する。
pub(super) fn contained_in(range: &[i32], span: Span) -> bool {
    let (start_line, start_col, end_line, end_col) = match range {
        [sl, sc, ec] => (*sl, *sc, *sl, *ec),
        [sl, sc, el, ec] => (*sl, *sc, *el, *ec),
        _ => return false,
    };
    let start = (start_line, start_col);
    let end = (end_line, end_col);
    // 終端が始端より前の範囲は壊れている。包含だけで判定すると通ってしまう。
    start <= end
        && start >= (span.start_line as i32, span.start_col as i32)
        && end <= (span.end_line as i32, span.end_col as i32)
}

/// 行に区切ったソース。UTF-16 変換と、未指定エンコーディングの判定にだけ使う。
pub(super) struct Lines<'a>(Vec<&'a [u8]>);

impl<'a> Lines<'a> {
    pub(super) fn of(content: &'a [u8]) -> Self {
        Lines(content.split(|&b| b == b'\n').collect())
    }

    fn get(&self, line: i32) -> Option<&'a [u8]> {
        usize::try_from(line)
            .ok()
            .and_then(|i| self.0.get(i))
            .copied()
    }
}

/// occurrence.range を、実際に使ってよいバイトオフセットとして得る。
///
/// Utf8 はそのまま借用で返す（Rust の常駐・速度を変えないため、ここで確保しない）。
/// Utf16 は行の内容を数えてバイトへ直す。Ambiguous（未指定）は、開始位置より前の
/// 部分が全部 ASCII ならバイトと UTF-16 の数え方が一致するのでそのまま使い、
/// そうでなければ None にする（索引だけではどちらの数え方か区別できないため）。
pub(super) fn usable_range<'a>(
    range: &'a [i32],
    encoding: scip_split::ColumnEncoding,
    lines: Option<&Lines<'_>>,
) -> Option<UsableRange<'a>> {
    use scip_split::ColumnEncoding::*;
    match encoding {
        Utf8 => Some(UsableRange::Borrowed(range)),
        Utf16 => convert_range(range, lines?).map(UsableRange::Owned),
        Ambiguous => {
            // 終端は見ない。語の中に非 ASCII があると終端の宣言値はバイト数より
            // 小さくなるが、ずれは必ず縮む向きなので範囲はその語自身に収まったまま。
            // Location は開始位置しか返さないので、他の語を指すことはない。
            let (line, start) = range_start(range)?;
            let text = lines?.get(line)?;
            let start = usize::try_from(start).ok()?;
            (start <= text.len() && text[..start].is_ascii())
                .then_some(UsableRange::Borrowed(range))
        }
    }
}

/// 変換が要らなければ借用のまま、要れば新しく確保して返す。
pub(super) enum UsableRange<'a> {
    Borrowed(&'a [i32]),
    Owned(Vec<i32>),
}

impl std::ops::Deref for UsableRange<'_> {
    type Target = [i32];
    fn deref(&self) -> &[i32] {
        match self {
            UsableRange::Borrowed(r) => r,
            UsableRange::Owned(r) => r,
        }
    }
}

fn range_start(range: &[i32]) -> Option<(i32, i32)> {
    match range {
        [line, start, ..] => Some((*line, *start)),
        _ => None,
    }
}

/// UTF-16 コードユニット単位の range を、行の内容を基準にバイトオフセットへ直す。
/// 行が無い・文字境界に乗らない場合は None（その occurrence は無かったことにする）。
fn convert_range(range: &[i32], lines: &Lines<'_>) -> Option<Vec<i32>> {
    let (sl, sc, el, ec, arity3) = match *range {
        [sl, sc, ec] => (sl, sc, sl, ec, true),
        [sl, sc, el, ec] => (sl, sc, el, ec, false),
        _ => return None,
    };
    let start = utf16_col_to_byte(lines.get(sl)?, sc)?;
    let end = utf16_col_to_byte(lines.get(el)?, ec)?;
    Some(if arity3 {
        vec![sl, start, end]
    } else {
        vec![sl, start, el, end]
    })
}

/// 行の中で、UTF-16 コードユニット `utf16_offset` 番目に対応するバイトオフセットを返す。
fn utf16_col_to_byte(line: &[u8], utf16_offset: i32) -> Option<i32> {
    let target = u32::try_from(utf16_offset).ok()?;
    let text = std::str::from_utf8(line).ok()?;
    let mut units = 0u32;
    for (byte_idx, ch) in text.char_indices() {
        if units == target {
            return i32::try_from(byte_idx).ok();
        }
        units += ch.len_utf16() as u32;
    }
    (units == target)
        .then(|| i32::try_from(text.len()).ok())
        .flatten()
}

pub(super) fn location_of(range: &[i32], rel: impl AsRef<Path>) -> Option<Location> {
    match range {
        [line, col, ..] => Some(Location {
            path: rel.as_ref().to_path_buf(),
            line: (*line).try_into().ok()?,
            col: (*col).try_into().ok()?,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::fixture::span;

    #[test]
    fn range_equal_to_the_word_is_contained() {
        assert!(contained_in(&[10, 4, 8], span(10, 4, 10, 8)));
        assert!(contained_in(&[10, 5, 7], span(10, 4, 10, 8)));
    }

    #[test]
    fn range_wider_than_the_word_is_not_contained() {
        assert!(!contained_in(&[10, 3, 8], span(10, 4, 10, 8)));
        assert!(!contained_in(&[10, 4, 9], span(10, 4, 10, 8)));
        // ファイル全体を覆う範囲
        assert!(!contained_in(&[0, 0, 299, 0], span(10, 4, 10, 8)));
    }

    #[test]
    fn range_on_another_line_is_not_contained() {
        assert!(!contained_in(&[11, 4, 8], span(10, 4, 10, 8)));
        assert!(!contained_in(&[10, 4, 11, 8], span(10, 4, 10, 8)));
    }

    #[test]
    fn malformed_range_is_contained_in_nothing() {
        assert!(!contained_in(&[], span(0, 0, 0, 1)));
        assert!(!contained_in(&[1, 2], span(1, 0, 1, 9)));
        // 終端が始端より前
        assert!(!contained_in(&[1, 8, 1], span(1, 0, 1, 13)));
        assert!(!contained_in(&[1, 8, 0, 3], span(0, 0, 2, 0)));
    }
}
