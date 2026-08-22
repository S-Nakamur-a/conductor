//! git の blob ハッシュ。索引の出自を申告するのに使う。

use sha1::{Digest, Sha1};

/// git の blob ハッシュ。索引の鍵を `git ls-files -s` の出力とそのまま突き合わせられるようにする。
/// [`Store::load`] の `expected` に入れるハッシュ。git の blob ハッシュと同じ形式なので、
/// `git ls-tree -r <索引を作ったコミット>` の出力をそのまま使える。
pub fn blob_hash(content: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(format!("blob {}\0", content.len()).as_bytes());
    hasher.update(content);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_hash_matches_git() {
        assert_eq!(
            blob_hash(b"hello\n"),
            "ce013625030ba8dba906f756967f9e9ca394464a"
        );
        assert_eq!(blob_hash(b""), "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
    }
}
