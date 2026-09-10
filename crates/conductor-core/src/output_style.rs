//! Claude Code の output style ファイルを、プロンプトに差せる本文にする。
//!
//! 読むのは設定が名指ししたファイル 1 つだけ。どのスタイルが選ばれているかを
//! settings.json の優先順位から解くことはしない — managed settings や
//! プラグイン提供のスタイルまで再現する羽目になり、Claude Code が層を増やす
//! たびに黙って古い答えを返す。

use std::path::Path;

/// 設定が指している口調。直接書いた文章が、ファイルより先。
///
/// 逆にすると、目の前の config に書いてある文章が黙って無視される。
pub fn resolve(inline: Option<&str>, path: Option<&Path>) -> Option<String> {
    match (inline.map(str::trim).filter(|s| !s.is_empty()), path) {
        (Some(text), Some(path)) => {
            log::info!("using the inline review style, not {}", path.display());
            Some(text.to_string())
        }
        (Some(text), None) => Some(text.to_string()),
        (None, Some(path)) => load(path),
        (None, None) => None,
    }
}

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

    #[test]
    fn 直接書いた文章はファイルより先() {
        let got = resolve(Some("短く書く"), Some(Path::new("/nonexistent/style.md")));
        assert_eq!(got.unwrap(), "短く書く");
    }

    /// 空文字を「指定された」と読むと、口調の欄が空のまま送られる。
    #[test]
    fn 空文字は指定が無いのと同じ() {
        assert!(resolve(Some("  \n"), None).is_none());
    }

    #[test]
    fn どちらも無ければ何も返さない() {
        assert!(resolve(None, None).is_none());
    }
}
