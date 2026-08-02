//! レビューコメントと walkthrough ステップのキーになる、リポジトリ相対パスの
//! 正規の綴り。
//!
//! これらのパスは FileDiff::path と突き合わせる。FileDiff::path は git2 が
//! そのまま返す値なので必ず素の形 (src/foo.rs — ./ なし、区切りの重複なし、
//! 末尾スラッシュなし) になる。同じファイルを別の綴りでキーにすると、解決に
//! 静かに失敗する: ファイルは一覧にちゃんと並んでいるのに、walkthrough の
//! ステップは「この差分には無い」と報告する。
//!
//! 書き込み側も読み出し側も [normalize] を通す。mcp-serve のツールは書き込み前に
//! 正規化し、ストアは行を読み戻すときにもう一度正規化するので、この仕組みが
//! 入る前に書かれた行もマイグレーション無しで解決できる。

/// リポジトリ相対パスを、git が使う綴りに書き換える。
///
/// 前後の空白、"." セグメント (先頭の ./ を含む)、空セグメント
/// (スラッシュの重複)、末尾のスラッシュを落とす。
///
/// これは綴りの修正であってバリデータではない。検証する側が見落とさないよう、
/// 次の 2 つは意図的にそのまま残す:
///
/// - ".." セグメントは保持する。ここで解決してしまうと、拒否しなければならない
///   パス (mcp_serve::reply::ensure_repo_relative) が無害な見た目に化ける。
/// - 先頭の "/" も保持する。絶対パスは絶対パスのまま同じ検査に引っかかり、
///   まったく別の場所を指す相対パスへ静かに格下げされることがない。
pub fn normalize(path: &str) -> String {
    let trimmed = path.trim();
    let mut out = String::with_capacity(trimmed.len());
    if trimmed.starts_with('/') {
        out.push('/');
    }
    let mut first = true;
    for segment in trimmed.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if !first {
            out.push('/');
        }
        out.push_str(segment);
        first = false;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_paths_are_left_alone() {
        assert_eq!(normalize("src/foo.rs"), "src/foo.rs");
        assert_eq!(normalize("Cargo.toml"), "Cargo.toml");
        assert_eq!(normalize("docs/設計 メモ.md"), "docs/設計 メモ.md");
    }

    #[test]
    fn dot_slash_doubled_slash_and_trailing_slash_are_dropped() {
        assert_eq!(normalize("./src/foo.rs"), "src/foo.rs");
        assert_eq!(normalize("././src//foo.rs"), "src/foo.rs");
        assert_eq!(normalize("src/foo.rs/"), "src/foo.rs");
        assert_eq!(normalize("  src/foo.rs  "), "src/foo.rs");
        assert_eq!(normalize("./"), "");
    }

    /// ".." は正規化を生き延びなければならない。これを拒否する検証は正規化後の形に
    /// 対して走るので、ここで解決してしまうと脱出するパスを通ってしまう形に
    /// 洗浄することになる。
    #[test]
    fn parent_dir_segments_survive() {
        assert_eq!(normalize("../secret"), "../secret");
        assert_eq!(normalize("a/../../b"), "a/../../b");
        assert_eq!(normalize("./../secret"), "../secret");
    }

    /// 同様に絶対パスは絶対パスのままにして、絶対パスを拒否する呼び出し側が
    /// ちゃんと拒否できるようにする。
    #[test]
    fn absolute_paths_stay_absolute() {
        assert_eq!(normalize("/etc/passwd"), "/etc/passwd");
        assert_eq!(normalize("/etc//passwd/"), "/etc/passwd");
    }
}
