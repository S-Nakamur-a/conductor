//! 開いているファイルの本文と、それをディスクから読む処理。
//!
//! git を一切経由しないので、`.git` の無いディレクトリでも未追跡のファイルでも
//! 同じように開ける。

use std::collections::HashMap;
use std::path::Path;

use conductor_core::symbol_index::CodeMask;
use conductor_core::test_run::{TestRun, scan_go_test_runs, scan_rust_test_runs};
use ratatui::style::Style;
use ratatui::text::Line;

use super::fold::{self, FoldRange};
use super::media;

/// 画面に出さないファイルの上限。
const MAX_BYTES: u64 = 5 * 1024 * 1024;

/// バイナリ判定のために覗く先頭バイト数。
const SNIFF_BYTES: usize = 8192;

/// 素のテキストとして開かない拡張子。画像は [media] が持つので重ねない。
const OPAQUE_EXTS: &[&str] = &[
    "ico", "tiff", "mp4", "mov", "webm", "avi", "mp3", "wav", "flac", "pdf", "zip", "gz", "tar",
    "xz", "zst", "woff", "woff2", "ttf", "otf", "so", "dylib", "wasm",
];

/// アクティブなタブの本文。
#[derive(Debug, Default)]
pub struct Content {
    pub lines: Vec<String>,
    /// 根からの相対パス。
    pub path: Option<String>,
    /// 「未選択」「空ファイル」「読めなかった」はどれも lines が空になる。
    /// これが無いと 3 つを見分けられず、失敗が黙って「未選択」に丸められる。
    pub error: Option<String>,
    /// syntect の結果。空ならハイライト無しで描く。
    pub highlighted: Vec<Vec<(Style, String)>>,
    pub(super) highlight_key: Option<u64>,
    /// 実行できるテストの行 (1 始まり)。行頭の \u{25b8} と、押したときに送る
    /// コマンドの両方がここを引く。
    pub tests: HashMap<usize, TestRun>,
    /// レンダリング済み markdown。折り返し済みで本文の行とは対応しないので、
    /// [Content::lines] とは別に持つ。
    pub(super) rendered: Vec<Line<'static>>,
    pub(super) rendered_key: Option<u64>,
    /// 画像のプレビュー。素のテキストを持たないファイルはこちらだけが埋まる。
    pub media: Option<media::Preview>,
    pub(super) media_key: Option<media::Key>,
}

/// ワーカーが 1 回の読み込みで作るもの。
#[derive(Debug)]
pub struct Loaded {
    /// タブ展開済み。
    pub lines: Vec<String>,
    pub folds: Vec<FoldRange>,
    /// どの語がコードでどれが地の文か。ジャンプもホバーもここを最初に通る。
    pub mask: CodeMask,
    pub tests: HashMap<usize, TestRun>,
}

/// root/relative を読む。折りたたみ範囲とコードマスクも同じ場所で求める
/// (どちらも tree-sitter を通すので、開くたびに 3 回パースしない)。
pub fn read(root: &Path, relative: &str, tab_width: usize) -> Result<Loaded, String> {
    let full = root.join(relative);
    if let Some(reason) = unsupported(&full, relative) {
        return Err(reason);
    }
    let text = std::fs::read_to_string(&full).map_err(|e| e.to_string())?;
    let mut lines: Vec<String> = text.lines().map(|l| expand_tabs(l, tab_width)).collect();
    if lines.is_empty() && !text.is_empty() {
        lines.push(String::new());
    }
    // 折りたたみは展開前のテキストから求める。tree-sitter もインデント幅も、
    // 書かれたままのファイルを前提にしている。
    let folds = fold::compute(&text, relative);
    let tests = if relative.ends_with(".rs") {
        scan_rust_test_runs(&lines, relative)
    } else {
        scan_go_test_runs(&lines, relative)
    };
    Ok(Loaded {
        lines,
        folds,
        mask: CodeMask::compute(&text, relative),
        tests,
    })
}

/// 素のテキストとして開けないなら、その理由。
fn unsupported(full: &Path, relative: &str) -> Option<String> {
    if let Some(ext) = extension(relative)
        && (OPAQUE_EXTS.contains(&ext.as_str()) || media::is_image_path(relative))
    {
        return Some(format!("{ext} files are not shown here"));
    }
    let size = std::fs::metadata(full).ok()?.len();
    if size > MAX_BYTES {
        return Some(format!("file is too large ({} MiB)", size / (1024 * 1024)));
    }
    contains_nul(full).then(|| "binary file".to_string())
}

fn contains_nul(full: &Path) -> bool {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(full) else {
        return false;
    };
    let mut head = [0u8; SNIFF_BYTES];
    match file.read(&mut head) {
        Ok(n) => head[..n].contains(&0),
        Err(_) => false,
    }
}

fn extension(path: &str) -> Option<String> {
    Path::new(path)
        .extension()?
        .to_str()
        .map(str::to_ascii_lowercase)
}

fn expand_tabs(line: &str, tab_width: usize) -> String {
    if !line.contains('\t') {
        return line.to_string();
    }
    let width = tab_width.max(1);
    let mut out = String::with_capacity(line.len());
    let mut col = 0;
    for ch in line.chars() {
        if ch == '\t' {
            let spaces = width - (col % width);
            out.extend(std::iter::repeat_n(' ', spaces));
            col += spaces;
        } else {
            out.push(ch);
            col += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> tempfile::TempDir {
        tempfile::TempDir::with_prefix(name).unwrap()
    }

    #[test]
    fn ファイルはgitを経ずにディスクから読む() {
        let dir = dir("read");
        std::fs::write(dir.path().join("plain.txt"), "ALPHA\nBRAVO\n").unwrap();
        assert!(!dir.path().join(".git").exists());

        let loaded = read(dir.path(), "plain.txt", 4).unwrap();
        assert_eq!(loaded.lines, ["ALPHA", "BRAVO"]);
    }

    #[test]
    fn 開けなかった理由が返る() {
        let dir = dir("errors");
        std::fs::write(dir.path().join("bin"), [0x7f, 0x45, 0x00, 0x01]).unwrap();
        std::fs::write(dir.path().join("logo.png"), "not really a png").unwrap();

        let cases = [
            ("missing.txt", "os error"),
            ("bin", "binary"),
            ("logo.png", "png"),
        ];
        for (path, needle) in cases {
            let err = read(dir.path(), path, 4).unwrap_err();
            assert!(err.contains(needle), "{path}: {err}");
        }
    }

    /// 読み込みと同時に走査する。開いた後で別に走らせると、ボタンの出る 1 フレームが遅れる。
    #[test]
    fn 実行できるテストは読み込みと同時に見つかる() {
        let dir = dir("tests");
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("src/lib.rs"),
            "#[cfg(test)]\nmod tests {\n    #[test]\n    fn works() {}\n}\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("src/plain.rs"), "fn main() {}\n").unwrap();

        let runs = read(dir.path(), "src/lib.rs", 4).unwrap().tests;
        assert_eq!(runs[&4].label, "works");
        assert!(runs[&4].command.starts_with("cargo test"), "{:?}", runs[&4]);
        assert!(runs.contains_key(&1), "ファイル全体のボタンも出る");

        assert!(
            read(dir.path(), "src/plain.rs", 4)
                .unwrap()
                .tests
                .is_empty()
        );
    }

    #[test]
    fn タブはタブストップまで広げる() {
        let cases = [
            ("\tx", 4, "    x"),
            ("ab\tx", 4, "ab  x"),
            ("abcd\tx", 4, "abcd    x"),
            ("no tabs", 4, "no tabs"),
        ];
        for (line, width, expected) in cases {
            assert_eq!(expand_tabs(line, width), expected, "{line:?}");
        }
    }
}
