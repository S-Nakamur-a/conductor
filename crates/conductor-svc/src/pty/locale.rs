//! 起動するエディタ向けの UTF-8 ロケール判定と、PTY へ書くときの UTF-8 安全な分割。

/// 継承した LC_ALL / LC_CTYPE / LANG から、起動するエディタに与える上書きを決める。
/// 返すのは (設定する変数, 削除する変数)。
///
/// 端末エディタは文字エンコーディングをロケールから導く。vim は UTF-8 ロケールが
/// 無ければ encoding=latin1 に落ち、日本語が入力時にも再読込時にも化ける。
pub(super) fn utf8_locale_overrides(
    lc_all: Option<&str>,
    lc_ctype: Option<&str>,
    lang: Option<&str>,
) -> (Vec<(&'static str, &'static str)>, Vec<&'static str>) {
    fn denotes_utf8(value: &str) -> bool {
        let value = value.to_ascii_lowercase();
        value.contains("utf-8") || value.contains("utf8")
    }
    // 空の値 (LANG=) は未設定と同じ。
    fn active(value: Option<&str>) -> Option<&str> {
        value.filter(|s| !s.is_empty())
    }

    // POSIX の優先順位。
    let effective = active(lc_all).or(active(lc_ctype)).or(active(lang));
    if effective.is_some_and(denotes_utf8) {
        return (Vec::new(), Vec::new());
    }

    // C.UTF-8 はロケール中立な UTF-8。macOS には別途インストールされたロケールとしては
    // 無いが、vim は encoding=utf-8 と解釈する。メッセージの言語を変えないよう文字
    // エンコーディングを司る LC_CTYPE だけを設定し、それを覆い隠す非 UTF-8 の LC_ALL は消す。
    let mut removes = Vec::new();
    if active(lc_all).is_some() {
        removes.push("LC_ALL");
    }
    (vec![("LC_CTYPE", "C.UTF-8")], removes)
}

/// text を、マルチバイト文字の途中で切らずに最大 max **バイト** の連続片へ分ける。
///
/// 固定オフセットで割ると文字の途中に着地することがあり、受け手は不完全な列を見て
/// 置換文字や文字化けを描く。
pub(super) fn utf8_chunks(text: &str, max: usize) -> Vec<&str> {
    assert!(max > 0, "chunk size must be positive");
    let mut chunks = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let mut end = max.min(rest.len());
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        // max が先頭の 1 文字より小さいときだけここに来る。丸ごと出して必ず前進する。
        if end == 0 {
            end = rest.chars().next().map_or(rest.len(), char::len_utf8);
        }
        let (chunk, tail) = rest.split_at(end);
        chunks.push(chunk);
        rest = tail;
    }
    chunks
}
