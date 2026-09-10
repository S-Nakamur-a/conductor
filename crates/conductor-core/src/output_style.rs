//! Claude Code の output style ファイルを、プロンプトに差せる本文にする。
//!
//! 読むのは設定が名指ししたファイル 1 つだけ。どのスタイルが選ばれているかを
//! settings.json の優先順位から解くことはしない — managed settings や
//! プラグイン提供のスタイルまで再現する羽目になり、Claude Code が層を増やす
//! たびに黙って古い答えを返す。

use std::path::Path;

/// frontmatter を落とした本文。読めない・空なら None。
///
/// 失敗を握り潰すのは、口調の指定が無いレビューは成立するため。呼ぶ側は
/// 解析を止めずに続ける。
pub fn load(path: &Path) -> Option<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let body = strip_frontmatter(&text).trim().to_string();
            if body.is_empty() {
                log::warn!("output style at {} is empty", path.display());
                return None;
            }
            Some(body)
        }
        Err(e) => {
            log::warn!("could not read the output style {}: {e}", path.display());
            None
        }
    }
}

/// 先頭の `---` で囲まれた YAML を落とす。閉じが無ければ frontmatter ではない。
fn strip_frontmatter(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("---\n") else {
        return text;
    };
    match rest.split_once("\n---\n") {
        Some((_, body)) => body,
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatterを落として本文だけ返す() {
        let text = "---\nname: Gyaru\ndescription: x\n---\n\n## 口調\n\nうちはこう喋る\n";
        assert_eq!(strip_frontmatter(text).trim(), "## 口調\n\nうちはこう喋る");
    }

    #[test]
    fn frontmatterが無ければそのまま返す() {
        assert_eq!(strip_frontmatter("## 口調\n本文"), "## 口調\n本文");
    }

    /// 閉じない `---` を frontmatter と読むと、本文を丸ごと捨てて口調が消える。
    #[test]
    fn 閉じない区切りは本文として残す() {
        let text = "---\nname: x\n\n本文";
        assert_eq!(strip_frontmatter(text), text);
    }

    #[test]
    fn 読めないパスは何も返さない() {
        assert!(load(Path::new("/nonexistent/style.md")).is_none());
    }
}
