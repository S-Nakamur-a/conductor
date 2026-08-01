//! 全文検索 (grep) エンジン。`.gitignore` を尊重してファイルを辿り、正規表現
//! またはリテラルのパターンを検索する。

use std::fs;
use std::path::Path;
use std::sync::mpsc;

use ignore::WalkBuilder;
use regex::RegexBuilder;

/// grep 検索で見つかったマッチ 1 件。
#[derive(Debug, Clone)]
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

/// バックグラウンドの検索スレッドから送られる進捗。
pub enum GrepProgress {
    /// 結果のひとかたまり (定期的に送られる)。
    Results(Vec<GrepMatch>),
    /// 検索完了。マッチの総数を持つ。
    Done(usize),
    /// エラーが発生した。
    Error(String),
}

/// この件数を超えたら検索を打ち切る。
const MAX_RESULTS: usize = 5000;
/// 結果を送るときの 1 回あたりの件数。
const BATCH_SIZE: usize = 50;

/// 指定したファイル一覧に対して全文検索を実行する (第 1 段階のインクリメンタル検索用)。
///
/// `run_search()` とほぼ同じだが、ディレクトリツリー全体を辿るのではなく
/// 渡されたファイルパスだけを検索する。
pub fn run_search_files(
    root: &Path,
    pattern: &str,
    regex_mode: bool,
    case_sensitive: bool,
    files: Vec<String>,
    tx: mpsc::Sender<GrepProgress>,
) {
    let root = root.to_path_buf();
    let pattern = pattern.to_string();

    std::thread::spawn(move || {
        let escaped = if regex_mode {
            pattern.clone()
        } else {
            regex::escape(&pattern)
        };
        let re = match RegexBuilder::new(&escaped)
            .case_insensitive(!case_sensitive)
            .build()
        {
            Ok(re) => re,
            Err(e) => {
                let _ = tx.send(GrepProgress::Error(format!("Invalid pattern: {e}")));
                return;
            }
        };

        let mut total = 0usize;
        let mut batch = Vec::with_capacity(BATCH_SIZE);

        for rel_path in &files {
            let abs_path = root.join(rel_path);
            let content = match fs::read_to_string(&abs_path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            for (line_idx, line) in content.lines().enumerate() {
                if let Some(m) = re.find(line) {
                    batch.push(GrepMatch {
                        file_path: rel_path.clone(),
                        line_number: line_idx + 1,
                        line_content: line.to_string(),
                        match_start: m.start(),
                        match_end: m.end(),
                    });

                    total += 1;

                    if batch.len() >= BATCH_SIZE {
                        if tx
                            .send(GrepProgress::Results(std::mem::take(&mut batch)))
                            .is_err()
                        {
                            return;
                        }
                        batch = Vec::with_capacity(BATCH_SIZE);
                    }

                    if total >= MAX_RESULTS {
                        if !batch.is_empty() {
                            let _ = tx.send(GrepProgress::Results(batch));
                        }
                        let _ = tx.send(GrepProgress::Done(total));
                        return;
                    }
                }
            }
        }

        if !batch.is_empty() {
            let _ = tx.send(GrepProgress::Results(batch));
        }
        let _ = tx.send(GrepProgress::Done(total));
    });
}

/// バックグラウンドスレッドで全文検索を実行する。
///
/// `root` は検索対象の worktree ディレクトリ。
/// `pattern` は検索クエリ (リテラルまたは正規表現)。
/// `regex_mode` はパターンを正規表現として解釈するかどうか。
/// `case_sensitive` は大文字小文字を区別するかどうか。
pub fn run_search(
    root: &Path,
    pattern: &str,
    regex_mode: bool,
    case_sensitive: bool,
    tx: mpsc::Sender<GrepProgress>,
) {
    let root = root.to_path_buf();
    let pattern = pattern.to_string();

    std::thread::spawn(move || {
        // 正規表現パターンをコンパイルする。
        let escaped = if regex_mode {
            pattern.clone()
        } else {
            regex::escape(&pattern)
        };
        let re = match RegexBuilder::new(&escaped)
            .case_insensitive(!case_sensitive)
            .build()
        {
            Ok(re) => re,
            Err(e) => {
                let _ = tx.send(GrepProgress::Error(format!("Invalid pattern: {e}")));
                return;
            }
        };

        let mut total = 0usize;
        let mut batch = Vec::with_capacity(BATCH_SIZE);

        let walker = WalkBuilder::new(&root)
            .hidden(true) // 隠しファイルは飛ばす
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .build();

        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            // ディレクトリは飛ばす。
            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                continue;
            }

            let path = entry.path();

            // ファイル内容を読む。バイナリファイルは飛ばす。
            let content = match fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue, // バイナリか読み取り不能
            };

            for (line_idx, line) in content.lines().enumerate() {
                if let Some(m) = re.find(line) {
                    let rel_path = match path.strip_prefix(&root) {
                        Ok(p) => p.to_string_lossy().to_string(),
                        Err(_) => path.to_string_lossy().to_string(),
                    };

                    batch.push(GrepMatch {
                        file_path: rel_path,
                        line_number: line_idx + 1,
                        line_content: line.to_string(),
                        match_start: m.start(),
                        match_end: m.end(),
                    });

                    total += 1;

                    if batch.len() >= BATCH_SIZE {
                        if tx
                            .send(GrepProgress::Results(std::mem::take(&mut batch)))
                            .is_err()
                        {
                            return; // 受信側が落ちた (検索がキャンセルされた)
                        }
                        batch = Vec::with_capacity(BATCH_SIZE);
                    }

                    if total >= MAX_RESULTS {
                        // 残っているぶんを送り切る。
                        if !batch.is_empty() {
                            let _ = tx.send(GrepProgress::Results(batch));
                        }
                        let _ = tx.send(GrepProgress::Done(total));
                        return;
                    }
                }
            }
        }

        // 残っているぶんを送り切る。
        if !batch.is_empty() {
            let _ = tx.send(GrepProgress::Results(batch));
        }
        let _ = tx.send(GrepProgress::Done(total));
    });
}
