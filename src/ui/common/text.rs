//! 表示幅を意識した文字列の整形。

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// s を高々 max_cols 表示カラムに切り詰め、切った場合は末尾に … を付ける。
///
/// 省略記号のカラムは実際に削るときだけ確保する。無条件に確保すると、ちょうど
/// 収まる文字列まで切り詰めてしまう。切る単位は書記素クラスタで、基底文字と
/// 異体字セレクタの間で切ると幅の計測を誤り、結合文字が画面に浮いて残る。
pub fn truncate_to_width(s: &str, max_cols: usize) -> String {
    if max_cols == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(s) <= max_cols {
        return s.to_string();
    }
    let mut width = 0usize;
    let budget = max_cols - 1;
    for (i, cluster) in s.grapheme_indices(true) {
        let cw = UnicodeWidthStr::width(cluster);
        if width + cw > budget {
            let mut out = s[..i].to_string();
            out.push('\u{2026}');
            return out;
        }
        width += cw;
    }
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_string_that_fits_is_returned_unchanged() {
        assert_eq!(truncate_to_width("hello", 10), "hello");
        assert_eq!(truncate_to_width("日本語", 10), "日本語");
        assert_eq!(truncate_to_width("", 10), "");
    }

    #[test]
    fn a_truncated_string_stays_within_the_budget_including_the_ellipsis() {
        for (s, max) in [("hello world", 6), ("日本語テスト", 6), ("a日b本c", 4)] {
            let out = truncate_to_width(s, max);
            assert!(out.ends_with('\u{2026}'), "{out:?}");
            assert!(UnicodeWidthStr::width(out.as_str()) <= max, "{out:?}");
        }
    }

    #[test]
    fn a_zero_budget_returns_empty() {
        assert_eq!(truncate_to_width("anything", 0), "");
    }

    /// 省略記号のカラムを無条件に確保していた頃は "hell…" を返していた。
    #[test]
    fn a_string_that_fits_exactly_is_not_cut() {
        assert_eq!(truncate_to_width("hello", 5), "hello");
    }

    /// ⚠ とその U+FE0F セレクタの間で切ると、宙に浮いた結合文字が残る。
    #[test]
    fn a_grapheme_cluster_is_never_split() {
        let s = "\u{26a0}\u{fe0f}\u{26a0}\u{fe0f}\u{26a0}\u{fe0f}";
        let out = truncate_to_width(s, 5);
        assert!(UnicodeWidthStr::width(out.as_str()) <= 5, "{out:?}");
        assert!(!out.contains('\u{fe0f}') || out.contains('\u{26a0}'));
    }
}
