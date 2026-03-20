//! File path detection from terminal output text.
//!
//! Extracts file paths (with optional line/column numbers) from terminal text,
//! typically compiler output, `grep` results, or editor-style references.

use std::path::Path;

use regex::Regex;

/// A file reference extracted from terminal output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileLink {
    /// The file path (relative or absolute).
    pub path: String,
    /// 1-indexed line number, if present.
    pub line: Option<usize>,
    /// 1-indexed column number, if present.
    pub col: Option<usize>,
    /// Byte offset of the path start within the source text.
    pub start: usize,
    /// Byte offset past the last character of the match.
    pub end: usize,
}

/// Detect file path references in a single line of text.
///
/// Returns all matches sorted by position. Paths are validated against the
/// given `worktree_root` — only matches whose resolved path exists on disk
/// are returned.
pub fn detect_file_links(text: &str, worktree_root: &Path) -> Vec<FileLink> {
    // Lazy-compile patterns.  The order matters: more specific patterns first.
    let patterns: &[&str] = &[
        // Rust / generic compiler: `--> src/app.rs:42:10`
        r#"-->\s+([^\s:]+):(\d+):(\d+)"#,
        // path:line:col
        r#"(?:^|[\s"'`(,])([./]?[a-zA-Z0-9_.][a-zA-Z0-9_./\-]*\.[a-zA-Z0-9]+):(\d+):(\d+)"#,
        // path:line
        r#"(?:^|[\s"'`(,])([./]?[a-zA-Z0-9_.][a-zA-Z0-9_./\-]*\.[a-zA-Z0-9]+):(\d+)"#,
        // bare path starting with `./` or `/`
        r#"(?:^|[\s"'`(,])([./][a-zA-Z0-9_./\-]*\.[a-zA-Z0-9]+)"#,
        // bare relative path containing `/` (e.g. `src/app.rs`)
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

            // The overall match end (including line:col suffixes).
            let match_end = caps.get(caps.len() - 1).unwrap().end();

            // Skip if this region overlaps with an already-detected link.
            if covered.iter().any(|&(s, e)| match_start < e && match_end > s) {
                continue;
            }

            // Parse optional line/col groups.
            let line = caps.get(2).and_then(|m| m.as_str().parse::<usize>().ok());
            let col = caps.get(3).and_then(|m| m.as_str().parse::<usize>().ok());

            // Resolve path and check existence.
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

/// Find the file link at a given byte offset (column) within the text.
pub fn file_link_at_offset(links: &[FileLink], offset: usize) -> Option<&FileLink> {
    links.iter().find(|l| offset >= l.start && offset < l.end)
}

/// Extract text from a single vt100 screen row.
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
        // Create test files.
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
    fn test_nonexistent_path_excluded() {
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

        // Offset within the file path should match.
        let link = file_link_at_offset(&links, links[0].start + 2);
        assert!(link.is_some());
        assert_eq!(link.unwrap().path, "src/app.rs");

        // Offset outside the match should return None.
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
