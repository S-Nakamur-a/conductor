//! 端末出力のテキストからファイルパスを検出する。
//!
//! 端末のテキスト (典型的にはコンパイラの出力、grep の結果、エディタ形式の
//! 参照) からファイルパスを抽出する。行番号・桁番号が付いていれば併せて取る。

use std::path::Path;

use regex::Regex;

/// 端末出力から抽出したファイル参照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileLink {
    /// ファイルパス (相対または絶対)。
    pub path: String,
    /// 1 始まりの行番号 (あれば)。
    pub line: Option<usize>,
    /// 1 始まりの桁番号 (あれば)。
    pub col: Option<usize>,
    /// 元テキスト内でのパス開始のバイトオフセット。
    pub start: usize,
    /// マッチ末尾の次を指すバイトオフセット。
    pub end: usize,
}

/// テキスト 1 行の中からファイルパスの参照を検出する。
///
/// 見つかったマッチを位置順に返す。パスは渡された worktree_root を基準に
/// 検証し、解決した先が実在するものだけを返す。
pub fn detect_file_links(text: &str, worktree_root: &Path) -> Vec<FileLink> {
    // パターンは必要時にコンパイルする。順番に意味があり、限定的なものが先。
    let patterns: &[&str] = &[
        // Rust など一般的なコンパイラ: "--> src/app.rs:42:10"
        r#"-->\s+([^\s:]+):(\d+):(\d+)"#,
        // path:line:col 形式
        r#"(?:^|[\s"'`(,])([./]?[a-zA-Z0-9_.][a-zA-Z0-9_./\-]*\.[a-zA-Z0-9]+):(\d+):(\d+)"#,
        // path:line 形式
        r#"(?:^|[\s"'`(,])([./]?[a-zA-Z0-9_.][a-zA-Z0-9_./\-]*\.[a-zA-Z0-9]+):(\d+)"#,
        // ./ または / で始まる裸のパス
        r#"(?:^|[\s"'`(,])([./][a-zA-Z0-9_./\-]*\.[a-zA-Z0-9]+)"#,
        // / を含む裸の相対パス (例: src/app.rs)
        r#"(?:^|[\s"'`(,])([a-zA-Z0-9_][a-zA-Z0-9_.\-]*/[a-zA-Z0-9_./\-]*\.[a-zA-Z0-9]+)"#,
    ];

    let mut links: Vec<FileLink> = Vec::new();
    let mut covered: Vec<(usize, usize)> = Vec::new();

    for pat in patterns {
        let re = Regex::new(pat).expect("invalid regex");
        for caps in re.captures_iter(text) {
            let m_path = caps.get(1).unwrap();
            let path_str = m_path.as_str();
            let match_start = m_path.start();

            // マッチ全体の終端 (line:col の接尾辞を含む)。
            let match_end = caps.get(caps.len() - 1).unwrap().end();

            // 既に検出したリンクと範囲が重なるものは飛ばす。
            if covered
                .iter()
                .any(|&(s, e)| match_start < e && match_end > s)
            {
                continue;
            }

            // 任意の line / col グループをパースする。
            let line = caps.get(2).and_then(|m| m.as_str().parse::<usize>().ok());
            let col = caps.get(3).and_then(|m| m.as_str().parse::<usize>().ok());

            // パスを解決して実在を確認する。
            let resolved = if Path::new(path_str).is_absolute() {
                Path::new(path_str).to_path_buf()
            } else {
                worktree_root.join(path_str)
            };

            if !resolved.is_file() {
                continue;
            }

            covered.push((match_start, match_end));
            links.push(FileLink {
                path: path_str.to_string(),
                line,
                col,
                start: match_start,
                end: match_end,
            });
        }
    }

    links.sort_by_key(|l| l.start);
    links
}

/// テキスト内の指定バイトオフセット (桁) にあるファイルリンクを探す。
pub fn file_link_at_offset(links: &[FileLink], offset: usize) -> Option<&FileLink> {
    links.iter().find(|l| offset >= l.start && offset < l.end)
}

/// vt100 スクリーンの 1 行からテキストを取り出す。
pub fn extract_row_text(screen: &vt100::Screen, row: u16, cols: u16) -> String {
    let mut text = String::with_capacity(cols as usize);
    for col in 0..cols {
        if let Some(cell) = screen.cell(row, col) {
            text.push_str(&cell.contents());
        } else {
            text.push(' ');
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_test_dir() -> TempDir {
        let dir = TempDir::new().unwrap();
        // テスト用のファイルを作る。
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/app.rs"), "fn main() {}").unwrap();
        fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();
        fs::write(dir.path().join("README.md"), "# readme").unwrap();
        dir
    }

    #[test]
    fn test_detect_relative_path() {
        let dir = setup_test_dir();
        let links = detect_file_links("opening src/app.rs now", dir.path());
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].path, "src/app.rs");
        assert_eq!(links[0].line, None);
    }

    #[test]
    fn test_detect_path_with_line() {
        let dir = setup_test_dir();
        let links = detect_file_links("error at src/app.rs:42", dir.path());
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].path, "src/app.rs");
        assert_eq!(links[0].line, Some(42));
        assert_eq!(links[0].col, None);
    }

    #[test]
    fn test_detect_path_with_line_col() {
        let dir = setup_test_dir();
        let links = detect_file_links("error at src/app.rs:42:10", dir.path());
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].path, "src/app.rs");
        assert_eq!(links[0].line, Some(42));
        assert_eq!(links[0].col, Some(10));
    }

    #[test]
    fn test_detect_compiler_arrow() {
        let dir = setup_test_dir();
        let links = detect_file_links("  --> src/main.rs:10:5", dir.path());
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].path, "src/main.rs");
        assert_eq!(links[0].line, Some(10));
        assert_eq!(links[0].col, Some(5));
    }

    #[test]
    fn test_detect_dot_slash_path() {
        let dir = setup_test_dir();
        let links = detect_file_links("reading ./src/app.rs ok", dir.path());
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].path, "./src/app.rs");
    }

    #[test]
    fn a_path_that_is_not_on_disk_is_not_linkified() {
        let dir = setup_test_dir();
        let links = detect_file_links("error in src/nonexistent.rs:10", dir.path());
        assert!(links.is_empty());
    }

    #[test]
    fn test_multiple_links() {
        let dir = setup_test_dir();
        let links = detect_file_links("src/app.rs:1 and src/main.rs:2", dir.path());
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].path, "src/app.rs");
        assert_eq!(links[1].path, "src/main.rs");
    }

    #[test]
    fn test_file_link_at_offset() {
        let dir = setup_test_dir();
        let text = "error at src/app.rs:42:10 done";
        let links = detect_file_links(text, dir.path());
        assert!(!links.is_empty());

        // ファイルパスの内側のオフセットならマッチするはず。
        let link = file_link_at_offset(&links, links[0].start + 2);
        assert!(link.is_some());
        assert_eq!(link.unwrap().path, "src/app.rs");

        // マッチの外のオフセットなら None になるはず。
        let link = file_link_at_offset(&links, text.len() - 1);
        assert!(link.is_none());
    }

    #[test]
    fn test_absolute_path() {
        let dir = setup_test_dir();
        let abs = dir.path().join("src/app.rs");
        let text = format!("opening {} now", abs.display());
        let links = detect_file_links(&text, dir.path());
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].line, None);
    }
}
