//! 会話ログ (.jsonl) に現れる URL を集める。画面に折り返して描かれた URL が本物かを、
//! ここで集めた URL と照らして確かめる。
//!
//! 行を JSON として解かずに生のバイトから拾う。JSON の文字列は `"` と `\` を逃がすが、
//! どちらも URL の字に入れないので、URL は逃がされずにそのまま並んでいる。

use std::collections::HashSet;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;

pub const SCHEMES: [&str; 2] = ["https://", "http://"];

/// URL の一部とみなす字。markdown の記号や括り (`*` `` ` `` `<` `>`) と、JSON が逃がす字は入れない。
pub fn is_url_char(c: char) -> bool {
    c.is_ascii_graphic() && !matches!(c, '*' | '`' | '<' | '>' | '"' | '\\')
}

/// 文の終わりの句読点と、URL の中に開きの無い閉じ括弧を末尾から落とす。
pub fn trim_url(url: &str) -> &str {
    let mut url = url;
    loop {
        let Some(last) = url.chars().next_back() else {
            return url;
        };
        let unbalanced = [('(', ')'), ('[', ']'), ('{', '}')]
            .iter()
            .any(|&(open, close)| {
                last == close && url.matches(close).count() > url.matches(open).count()
            });
        if !matches!(last, '.' | ',' | ';' | ':' | '!' | '?' | '\'') && !unbalanced {
            return url;
        }
        url = &url[..url.len() - 1];
    }
}

/// `text` の中の URL。それぞれ trim_url を通したもの。
pub fn extract(text: &str) -> impl Iterator<Item = &str> {
    let mut rest = text;
    std::iter::from_fn(move || {
        loop {
            let tail = &rest[rest.find("http")?..];
            if !SCHEMES.iter().any(|s| tail.starts_with(s)) {
                rest = &tail["http".len()..];
                continue;
            }
            let len = tail.find(|c: char| !is_url_char(c)).unwrap_or(tail.len());
            rest = &tail[len..];
            let url = trim_url(&tail[..len]);
            if !SCHEMES.contains(&url) {
                return Some(url);
            }
        }
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scan {
    pub offset: u64,
    pub urls: HashSet<String>,
}

/// 読んだ所から先だけを読む。書きかけの最終行は次の呼び出しで読む。
pub fn scan(path: &Path, from: Scan) -> io::Result<Scan> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let Scan {
        mut offset,
        mut urls,
    } = if len < from.offset {
        Scan::default()
    } else {
        from
    };
    file.seek(SeekFrom::Start(offset))?;
    let mut reader = BufReader::new(file);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let read = reader.read_until(b'\n', &mut buf)?;
        if read == 0 || buf.last() != Some(&b'\n') {
            return Ok(Scan { offset, urls });
        }
        offset += read as u64;
        urls.extend(extract(&String::from_utf8_lossy(&buf)).map(str::to_string));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls(text: &str) -> Vec<&str> {
        extract(text).collect()
    }

    #[test]
    fn markdownの記号で囲んだurlは記号を含めない() {
        assert_eq!(
            urls("see **https://a.example/x** and `https://b.example/y` and <https://c.example/z>"),
            vec![
                "https://a.example/x",
                "https://b.example/y",
                "https://c.example/z"
            ]
        );
    }

    #[test]
    fn スキームでないhttpの後ろのurlも拾う() {
        assert_eq!(
            urls("httpd, httphttps://a.example/x and http://b.example/y"),
            vec!["https://a.example/x", "http://b.example/y"]
        );
    }

    #[test]
    fn 文末の句読点と対にならない括弧は落とす() {
        assert_eq!(
            urls("(see https://a.example/x). Also https://b.example/y, ok"),
            vec!["https://a.example/x", "https://b.example/y"]
        );
    }

    #[test]
    fn 対にならない角括弧と波括弧は落とし対は残す() {
        assert_eq!(
            urls("[https://a.ex/x] {https://b.ex/y} https://c.ex/p[1]"),
            vec!["https://a.ex/x", "https://b.ex/y", "https://c.ex/p[1]"]
        );
    }

    #[test]
    fn urlの中の括弧の対は残す() {
        assert_eq!(
            urls("https://en.wikipedia.org/wiki/Rust_(programming_language)."),
            vec!["https://en.wikipedia.org/wiki/Rust_(programming_language)"]
        );
    }

    #[test]
    fn jsonの文字列の中の改行の手前で切る() {
        let line = r#"{"text":"open https://a.example/x\nnext \"https://b.example/y\""}"#;
        assert_eq!(
            urls(line),
            vec!["https://a.example/x", "https://b.example/y"]
        );
    }

    #[test]
    fn 読んだ所から先だけを読み書きかけの行は残す() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        std::fs::write(
            &path,
            "{\"t\":\"https://a.example/1\"}\n{\"t\":\"https://b.ex",
        )
        .unwrap();
        let first = scan(&path, Scan::default()).unwrap();
        assert_eq!(
            first.urls,
            HashSet::from(["https://a.example/1".to_string()])
        );

        std::fs::write(
            &path,
            "{\"t\":\"https://a.example/1\"}\n{\"t\":\"https://b.example/2\"}\n",
        )
        .unwrap();
        let second = scan(&path, first).unwrap();
        assert!(second.urls.contains("https://b.example/2"));
        assert_eq!(second.urls.len(), 2);
    }
}
