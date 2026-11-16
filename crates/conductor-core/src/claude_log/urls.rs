//! 会話ログ (.jsonl) に現れる URL とパスを集める。画面に折り返して描かれた URL が本物かを、
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

/// `tail` の頭からリンクになりうる並びが始まるか。URL、絶対パス、`localhost:4111` のような
/// スキームの無いローカルのホストとポート。後の 2 つは語の頭 (`after_break`) でだけ始まる。
pub fn starts_link(after_break: bool, tail: &str) -> bool {
    SCHEMES.iter().any(|s| tail.starts_with(s))
        || after_break && (starts_path(tail) || is_local_host(tail))
}

fn starts_path(tail: &str) -> bool {
    let mut chars = tail.chars();
    chars.next() == Some('/') && chars.next().is_some_and(|c| is_url_char(c) && c != '/')
}

fn is_local_host(text: &str) -> bool {
    let host_len = text
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '-'))
        .unwrap_or(text.len());
    let host = &text[..host_len];
    let local =
        matches!(host, "localhost" | "127.0.0.1" | "0.0.0.0") || host.ends_with(".localhost");
    local
        && text[host_len..]
            .strip_prefix(':')
            .is_some_and(|port| port.starts_with(|c: char| c.is_ascii_digit()))
}

/// 押したときに開く先。パスは在るものだけを file:// で返し、`:12` や `:12:5` の行番号は外す。
pub fn target(link: &str) -> Option<String> {
    if SCHEMES.iter().any(|s| link.starts_with(s)) {
        return (!SCHEMES.contains(&link)).then(|| link.to_string());
    }
    if starts_path(link) {
        let file = without_line_number(link);
        return Path::new(file).exists().then(|| format!("file://{file}"));
    }
    is_local_host(link).then(|| format!("http://{link}"))
}

fn without_line_number(path: &str) -> &str {
    let mut path = path;
    for _ in 0..2 {
        match path.rsplit_once(':') {
            Some((head, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => {
                path = head
            }
            _ => break,
        }
    }
    path
}

/// `text` の中のリンクになりうる並び。それぞれ trim_url を通したもの。
/// 会話ログの JSON では行の頭のパスが `\n` の直後に来るので、逃がした改行とタブも語の切れ目とみなす。
pub fn extract(text: &str) -> impl Iterator<Item = &str> {
    let mut at = 0;
    std::iter::from_fn(move || {
        while let Some(c) = text[at..].chars().next() {
            let tail = &text[at..];
            if starts_link(after_break(&text[..at]), tail) {
                let len = tail.find(|c: char| !is_url_char(c)).unwrap_or(tail.len());
                at += len;
                let link = trim_url(&tail[..len]);
                if starts_link(true, link) && !SCHEMES.contains(&link) {
                    return Some(link);
                }
                continue;
            }
            at += c.len_utf8();
        }
        None
    })
}

fn after_break(before: &str) -> bool {
    let mut back = before.chars().rev();
    match (back.next(), back.next()) {
        (None, _) => true,
        (Some('n' | 't' | 'r'), Some('\\')) => true,
        (Some(c), _) => !is_url_char(c),
    }
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
    fn スキームの無いローカルのホストとポートも拾い語の途中からは拾わない() {
        assert_eq!(
            urls("`localhost:4111` en.localhost:3000/a, 127.0.0.1:8080. localhost: notlocalhost:1"),
            vec!["localhost:4111", "en.localhost:3000/a", "127.0.0.1:8080"]
        );
        assert_eq!(
            urls("http://localhost:4111/x"),
            vec!["http://localhost:4111/x"]
        );
    }

    #[test]
    fn 語の頭とjsonの逃がした改行の後の絶対パスを拾う() {
        let line = r#"{"t":"see /Users/a/b.rs:12, src/c.rs\n/tmp/d //e /"}"#;
        assert_eq!(urls(line), vec!["/Users/a/b.rs:12", "/tmp/d"]);
    }

    #[test]
    fn 開く先はパスなら在るものだけで行番号を外しホストにはスキームを付ける() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "").unwrap();
        let file = file.to_str().unwrap();
        assert_eq!(
            target(&format!("{file}:12:5")),
            Some(format!("file://{file}"))
        );
        assert_eq!(target(&format!("{file}x")), None);
        assert_eq!(
            target("localhost:4111/x"),
            Some("http://localhost:4111/x".to_string())
        );
        assert_eq!(target("https://"), None);
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
