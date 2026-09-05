//! レビューコメントのキーになるリポジトリ相対パスの綴り。
//! FileDiff::path (git2 がそのまま返す形) と突き合わせるので、書く側も読む側も
//! [normalize] を通す。

/// 前後の空白、"." セグメント、空セグメント、末尾のスラッシュを落として git の綴りにする。
///
/// ".." と先頭の "/" は意図的に残す。ここで解決すると、拒否すべきパスが検証側で
/// 無害な見た目に化けるため。
pub fn normalize(path: &str) -> String {
    let trimmed = path.trim();
    let mut out = String::with_capacity(trimmed.len());
    if trimmed.starts_with('/') {
        out.push('/');
    }
    let segments = trimmed.split('/').filter(|s| !s.is_empty() && *s != ".");
    for (i, segment) in segments.enumerate() {
        if i > 0 {
            out.push('/');
        }
        out.push_str(segment);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn 正規化() {
        let cases = [
            ("src/foo.rs", "src/foo.rs"),
            ("Cargo.toml", "Cargo.toml"),
            ("docs/設計 メモ.md", "docs/設計 メモ.md"),
            ("./src/foo.rs", "src/foo.rs"),
            ("././src//foo.rs", "src/foo.rs"),
            ("src/foo.rs/", "src/foo.rs"),
            ("  src/foo.rs  ", "src/foo.rs"),
            ("./", ""),
            ("../secret", "../secret"),
            ("a/../../b", "a/../../b"),
            ("./../secret", "../secret"),
            ("/etc/passwd", "/etc/passwd"),
            ("/etc//passwd/", "/etc/passwd"),
        ];
        for (input, want) in cases {
            assert_eq!(normalize(input), want, "input={input:?}");
        }
    }
}
