//! 全文検索 (grep) の本体。.gitignore を尊重してファイルを辿り、正規表現または
//! リテラルパターンで検索する。
//!
//! バックグラウンドスレッドでの実行と結果の逐次配信は呼び出し側 (svc) の責務。
//! ここが公開するのは同期関数だけ: 1 ファイルを検索する [search_file] と、
//! ファイル一覧またはツリー全体を検索する [search_files] / [search_tree]。

use std::fs;
use std::path::Path;

use ignore::WalkBuilder;
use regex::{Regex, RegexBuilder};

#[cfg(test)]
mod tests;

/// grep 検索で見つかったマッチ 1 件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrepMatch {
    /// worktree ルートからの相対ファイルパス。
    pub file_path: String,
    /// 1 始まりの行番号。
    pub line_number: usize,
    /// マッチした行の全内容。
    pub line_content: String,
    /// 行内でのマッチ開始のバイトオフセット。
    pub match_start: usize,
    /// 行内でのマッチ終了のバイトオフセット。
    pub match_end: usize,
}

/// 1 回の検索で返すマッチ数の上限。リポジトリ全体を無制限に読み切ろうとしないための
/// 安全弁で、これに達したら [search_files] / [search_tree] はそこで打ち切る。
pub const MAX_RESULTS: usize = 5000;

/// pattern を検索用の正規表現にコンパイルする。regex_mode が false ならリテラルとして
/// エスケープしてから組み立てる。
pub fn compile_pattern(
    pattern: &str,
    regex_mode: bool,
    case_sensitive: bool,
) -> Result<Regex, regex::Error> {
    let escaped = if regex_mode {
        pattern.to_string()
    } else {
        regex::escape(pattern)
    };
    RegexBuilder::new(&escaped)
        .case_insensitive(!case_sensitive)
        .build()
}

/// 1 ファイルを検索する。`rel_path` は root からの相対パスで、そのままマッチに刻む。
/// 1 行につき最初のマッチだけを拾う。バイナリなどで読めないファイルは空を返す。
pub fn search_file(root: &Path, rel_path: &str, re: &Regex) -> Vec<GrepMatch> {
    let Ok(content) = fs::read_to_string(root.join(rel_path)) else {
        return Vec::new();
    };
    content
        .lines()
        .enumerate()
        .filter_map(|(idx, line)| {
            re.find(line).map(|m| GrepMatch {
                file_path: rel_path.to_string(),
                line_number: idx + 1,
                line_content: line.to_string(),
                match_start: m.start(),
                match_end: m.end(),
            })
        })
        .collect()
}

/// 指定したファイル一覧だけを検索する (インクリメンタル検索の第 1 段階向け)。
pub fn search_files(root: &Path, rel_paths: &[String], re: &Regex) -> Vec<GrepMatch> {
    let mut results = Vec::new();
    for rel_path in rel_paths {
        results.extend(search_file(root, rel_path, re));
        if results.len() >= MAX_RESULTS {
            results.truncate(MAX_RESULTS);
            break;
        }
    }
    results
}

/// root 以下を .gitignore を尊重して走査し、全文検索する。
pub fn search_tree(root: &Path, re: &Regex) -> Vec<GrepMatch> {
    let walker = WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .build();

    let mut results = Vec::new();
    for entry in walker {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let path = entry.path();
        let rel_path = path.strip_prefix(root).unwrap_or(path).to_string_lossy();

        results.extend(search_file(root, &rel_path, re));
        if results.len() >= MAX_RESULTS {
            results.truncate(MAX_RESULTS);
            break;
        }
    }
    results
}
