//! 起動するエディタ向けの UTF-8 ロケール上書き検出と、PTY へ大きなペイロードを
//! 書き込む際に使う UTF-8 安全なバイトチャンク分割。

/// 継承された LC_ALL / LC_CTYPE / LANG の値から、起動するエディタが
/// I/O を UTF-8 として扱うために必要なロケール環境の上書きを決める。
///
/// 端末エディタは文字エンコーディングをロケールから導出する。例えば vim は
/// UTF-8 ロケールが有効でない場合 encoding=latin1 にフォールバックし、
/// 全角文字やマルチバイト(日本語など)テキストが入力時にも、ファイル再読込
/// 時にも化ける。
///
/// (sets, removes) を返す: 子コマンドに設定すべき環境変数と、削除すべき
/// 環境変数。すでに UTF-8 ロケールが有効な場合はユーザーの設定を尊重し、
/// 両方とも空のリストになる。
pub(super) fn utf8_locale_overrides(
    lc_all: Option<&str>,
    lc_ctype: Option<&str>,
    lang: Option<&str>,
) -> (Vec<(&'static str, &'static str)>, Vec<&'static str>) {
    fn denotes_utf8(value: &str) -> bool {
        let value = value.to_ascii_lowercase();
        value.contains("utf-8") || value.contains("utf8")
    }
    // 空の値(LANG=)は変数が未設定なのと等価。
    fn active(value: Option<&str>) -> Option<&str> {
        value.filter(|s| !s.is_empty())
    }

    // POSIX の優先順位: LC_ALL は LC_CTYPE より優先され、LC_CTYPE は LANG より優先される。
    let effective = active(lc_all).or(active(lc_ctype)).or(active(lang));
    if effective.is_some_and(denotes_utf8) {
        return (Vec::new(), Vec::new());
    }

    // C.UTF-8 はロケール中立な UTF-8 ロケールである: 最近の Linux には
    // 存在し、macOS では別途インストールされたロケールではないにもかかわらず
    // vim には encoding=utf-8 として解釈される。エディタのメッセージ言語を
    // 変えないよう、文字エンコーディングを司るカテゴリである LC_CTYPE だけを
    // 設定する。非 UTF-8 の LC_ALL があるとそれを覆い隠してしまうため、
    // 存在する場合は削除する。
    let mut removes = Vec::new();
    if active(lc_all).is_some() {
        removes.push("LC_ALL");
    }
    (vec![("LC_CTYPE", "C.UTF-8")], removes)
}

/// text を、マルチバイト UTF-8 文字の途中を絶対に切らない形で、最大 max
/// **バイト** 長の連続した部分スライスに分割する。
///
/// PTY の writer は、カーネルの PTY 入力バッファに収まるよう大きなペイロードを
/// チャンク分割する(フラッシュを挟み、チャンク上限に達したときはチャンク間に
/// 小さな遅延を入れる)。生のバイトスライスを固定オフセットで分割すると
/// マルチバイト文字の途中に着地することがあり、受け手のアプリケーションは
/// 不完全なシーケンスを見て置換文字や文字化けを描画してしまう。分割位置を
/// 最も近い文字境界まで戻すことで、全角文字やマルチバイト入力を無傷に保つ。
pub(super) fn utf8_chunks(text: &str, max: usize) -> Vec<&str> {
    assert!(max > 0, "chunk size must be positive");
    let mut chunks = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let mut end = max.min(rest.len());
        // end が文字境界に来るまで後退する(rest.len() では必ず境界に
        // なるので、これは必ず終了する)。
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        // 防御的処置: max が最初の文字より小さい場合にのみ到達しうる
        // (ここで使う 1 KiB のチャンクサイズでは起こらない)。その文字を
        // 丸ごと出力して必ず前進するようにする。
        if end == 0 {
            end = rest.chars().next().map_or(rest.len(), char::len_utf8);
        }
        let (chunk, tail) = rest.split_at(end);
        chunks.push(chunk);
        rest = tail;
    }
    chunks
}
