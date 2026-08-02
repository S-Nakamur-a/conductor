//! git2/similar ベースの diff 計算。リポジトリからの DiffState の読み込み、
//! ファイル単位のハンク/行の構築、関数コンテキストヘッダーの検出、
//! 大文字小文字の違いだけのリネームのフィルタリング(大文字小文字を区別しない
//! ファイルシステム向け)を行う。

use std::path::Path;

use anyhow::{Context, Result};
use git2::Repository;
use regex::Regex;
use similar::{ChangeTag, TextDiff};

use super::model::{
    DiffHunk, DiffLine, DiffLineTag, DiffRange, DiffState, FileDiff, InlineSegment,
};

/// base(ブランチ名、リモート追跡 ref、タグ、または生の OID)を diff の
/// 基準となるコミットに解決する。
///
/// find_branch ではなく revparse_single を使う理由: Conductor が作成する
/// worktree はベースを origin/<main> として記録しており(GitEngine::resolve_base_ref
/// 参照)、リモート追跡 ref はローカルブランチではない。この不一致が変更ファイル
/// リストを空で返す原因になっていた。
///
/// 名前は記録された通りに厳密に解決するので、ベースが origin/main であれば
/// 古いローカルの main が存在してもリモート追跡 ref の方を使う。逆に PR intake
/// は PR のベースを裸の名前(main)として記録しており、こちらはローカルブランチに
/// 解決される(リモートより遅れている可能性がある)。origin/ を付けての再試行は、
/// develop のように refs/remotes/origin/develop としてしか存在しないローカル
/// ref を持たない名前のためだけに行う。
///
/// 解決の優先順位は git 自体の revspec ルールに従うので、main という名前の
/// タグは main という名前のブランチより優先される。これは git rev-parse main
/// が選ぶものと一致する。
fn resolve_base_commit(repo: &Repository, base: &str) -> Result<git2::Oid> {
    let resolved = repo.revparse_single(base).or_else(|primary| {
        // 裸の名前(develop)で記録されたベースは refs/remotes/origin/develop
        // としてしか存在しないことがある。git の revspec ルールは
        // refs/remotes/<name> までしか探さず、リモートを自動補完してはくれない。
        // 既に修飾済みの名前では再試行をスキップし、ユーザが読むエラーが
        // 決して origin/origin/main にならないようにする。原因としては
        // 最初の失敗を保持する。それがユーザが実際に設定した ref を指しているからだ。
        if base.starts_with("origin/") {
            return Err(primary);
        }
        repo.revparse_single(&format!("origin/{base}"))
            .map_err(|_| primary)
    });
    let obj = resolved.with_context(|| format!("base ref '{base}' cannot be resolved"))?;
    let commit = obj
        .peel_to_commit()
        .with_context(|| format!("cannot peel '{base}' to commit"))?;
    Ok(commit.id())
}

impl DiffState {
    /// worktree_path のリポジトリについて base_branch と HEAD の diff を読み込み、
    /// 以前に保持していた diff データを置き換える。
    ///
    /// コミット済み(merge-base..HEAD)と未コミット(HEAD vs workdir+index)の
    /// 両方の diff を計算する。
    pub fn load_diff(
        &mut self,
        worktree_path: &Path,
        base_branch: &str,
        word_diff: bool,
        tab_width: usize,
    ) {
        self.base_branch = base_branch.to_string();
        self.error = None;

        // コミット済みの diff を計算する。
        match Self::compute_diff_range(
            worktree_path,
            base_branch,
            DiffRange::Committed,
            word_diff,
            tab_width,
        ) {
            Ok(mut files) => {
                files.sort_by(|a, b| a.path.cmp(&b.path));
                self.committed_files = files;
            }
            Err(e) => {
                self.committed_files.clear();
                self.error = Some(format!("{e:#}"));
                // 下の未コミット計算にはそのまま進む。未コミット側は base_branch
                // に依存しないので、不正なベースが未コミットの変更まで隠してしまっては
                // ならない。
            }
        }

        // 未コミットの diff を計算する。
        match Self::compute_diff_range(
            worktree_path,
            base_branch,
            DiffRange::Uncommitted,
            word_diff,
            tab_width,
        ) {
            Ok(mut files) => {
                files.sort_by(|a, b| a.path.cmp(&b.path));
                self.uncommitted_files = files;
            }
            Err(e) => {
                self.uncommitted_files.clear();
                // 致命的ではない: コミット済みの diff は正常に読み込めている。
                log::warn!("failed to compute uncommitted diff: {e:#}");
            }
        }

        self.rebuild_display_list();
        self.scroll = 0;
    }

    /// ファイル拡張子に基づいて、関数/クラス/構造体のヘッダーを検出するための
    /// 正規表現パターンを返す。対応していない拡張子には None を返す。
    fn func_pattern_for_ext(ext: &str) -> Option<Regex> {
        let pattern = match ext {
            // Rust
            "rs" => r"^\s*(pub\s+)?(async\s+)?(fn|impl|struct|enum|trait|mod|macro_rules!)\b",
            // TypeScript / JavaScript
            "ts" | "tsx" | "js" | "jsx" | "mjs" | "mts" | "cjs" | "cts" => {
                r"^\s*(export\s+)?(default\s+)?(async\s+)?(function\*?|class)\b|^\s*(export\s+)?(const|let|var)\s+\w+\s*="
            }
            // Python
            "py" => r"^\s*(async\s+)?(def|class)\b",
            // Go
            "go" => r"^(func|type)\b",
            // Java / C# / Kotlin
            "java" | "cs" | "kt" | "kts" => {
                r"^\s*(public|private|protected|internal|static|abstract|override|final|suspend)?\s*(public|private|protected|internal|static|abstract|override|final|suspend)?\s*(class|interface|enum|record|fun|void|int|long|string|bool|boolean|var|val|object)\b"
            }
            // C / C++
            "c" | "h" | "cpp" | "cc" | "cxx" | "hpp" | "hxx" => {
                r"^[a-zA-Z_][\w:*&<> ]*\s+\*?\w+\s*\(|^\s*(class|struct|enum|namespace|template)\b"
            }
            // Ruby
            "rb" => r"^\s*(def|class|module)\b",
            // PHP
            "php" => r"^\s*(public|private|protected|static)?\s*(function|class|interface|trait)\b",
            // Shell
            "sh" | "bash" | "zsh" => r"^\s*(\w+\s*\(\)|function\s+\w+)",
            _ => return None,
        };
        Regex::new(pattern).ok()
    }

    /// start_line(0始まり)から上方向に走査し、旧ファイル内容から最も近い
    /// 関数ヘッダーを見つける。
    fn find_func_header(old_lines: &[&str], start_line: usize, pattern: &Regex) -> Option<String> {
        for i in (0..=start_line).rev() {
            let line = old_lines[i].trim_end();
            if pattern.is_match(line) {
                // 表示用に、長すぎるヘッダーは切り詰める。
                let trimmed = line.trim();
                let header = if trimmed.len() > 80 {
                    // バイト位置80以下で最後の文字境界を探す。
                    let mut end = 80;
                    while !trimmed.is_char_boundary(end) {
                        end -= 1;
                    }
                    format!("{}…", &trimmed[..end])
                } else {
                    trimmed.to_string()
                };
                return Some(header);
            }
        }
        None
    }

    /// バックグラウンドでの diff 計算用の公開ラッパー。
    ///
    /// committed: true なら merge-base..HEAD、false なら HEAD vs workdir+index を計算する。
    pub fn compute_diff_range_static(
        worktree_path: &Path,
        base_branch: &str,
        committed: bool,
        word_diff: bool,
        tab_width: usize,
    ) -> Result<Vec<FileDiff>> {
        let range = if committed {
            DiffRange::Committed
        } else {
            DiffRange::Uncommitted
        };
        Self::compute_diff_range(worktree_path, base_branch, range, word_diff, tab_width)
    }

    /// git2 + similar を使い、指定した範囲についてファイル単位の diff を計算する。
    pub(super) fn compute_diff_range(
        worktree_path: &Path,
        base_branch: &str,
        range: DiffRange,
        word_diff: bool,
        tab_width: usize,
    ) -> Result<Vec<FileDiff>> {
        let repo = Repository::open(worktree_path)
            .with_context(|| format!("cannot open repo at {}", worktree_path.display()))?;

        // HEAD を解決する。
        let head_commit = repo
            .head()
            .with_context(|| "cannot resolve HEAD")?
            .peel_to_commit()
            .with_context(|| "cannot peel HEAD to commit")?;
        let head_oid = head_commit.id();

        // range に応じて git2 の diff を構築する。
        let diff = match range {
            DiffRange::Committed => {
                // merge-base(base, HEAD)..HEAD
                let base_oid = resolve_base_commit(&repo, base_branch)?;
                let merge_base_oid = repo.merge_base(base_oid, head_oid).with_context(|| {
                    format!("cannot find merge-base between '{base_branch}' and HEAD")
                })?;
                let merge_base_tree = repo
                    .find_commit(merge_base_oid)?
                    .tree()
                    .with_context(|| "cannot get merge-base tree")?;
                let head_tree = head_commit.tree()?;
                repo.diff_tree_to_tree(Some(&merge_base_tree), Some(&head_tree), None)?
            }
            DiffRange::Uncommitted => {
                // HEAD..workdir+index
                let head_tree = head_commit.tree()?;
                let mut opts = git2::DiffOptions::new();
                opts.include_untracked(true);
                opts.recurse_untracked_dirs(true);
                repo.diff_tree_to_workdir_with_index(Some(&head_tree), Some(&mut opts))?
            }
        };

        // workdir から読む必要があるか(未ステージ/未追跡ファイル向け)を判定する。
        let use_workdir = range == DiffRange::Uncommitted;

        let mut file_diffs = Vec::new();

        // スキップするデルタのインデックス集合を作る: 大文字小文字だけ異なり
        // 内容が同一のパス。大文字小文字を区別しないファイルシステム(macOS)では、
        // ファイル内容が同一でもパスの大文字小文字だけが異なる削除+追加のペア
        // (例: "Photo.png" 削除、"photo.png" 追加)を git が報告することがある。
        // blob の OID と小文字化したパスを比較してこれらのペアを検出する。
        let skip_indices = Self::find_case_only_rename_indices(&diff);

        let num_deltas = diff.deltas().len();
        for delta_idx in 0..num_deltas {
            if skip_indices.contains(&delta_idx) {
                continue;
            }

            let delta = diff.get_delta(delta_idx).unwrap();

            // ファイルパスを決定する。
            let path = delta
                .new_file()
                .path()
                .or_else(|| delta.old_file().path())
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "(unknown)".to_string());

            // blob から旧内容を取得する。
            let old_content = Self::blob_content(&repo, &delta.old_file());

            // 新内容を取得する: workdir との diff では、blob id がゼロ
            // (未ステージ/未追跡)の場合はディスクから読む。
            let new_content = if use_workdir && delta.new_file().id().is_zero() {
                let full_path = worktree_path.join(&path);
                match std::fs::read(&full_path) {
                    Ok(bytes) => String::from_utf8(bytes)
                        .unwrap_or_else(|e| String::from_utf8_lossy(&e.into_bytes()).to_string()),
                    Err(_) => String::new(),
                }
            } else {
                Self::blob_content(&repo, &delta.new_file())
            };

            // リネーム検出が削除+追加を1つのデルタに統合した場合の、単一デルタでの
            // 大文字小文字違いリネームもスキップする。
            if Self::is_case_only_rename(&delta) && old_content == new_content {
                continue;
            }

            // 実質的な内容変更のないファイルはスキップする。
            // 大文字小文字を区別しないファイルシステムの stat 不一致による、
            // 偽のデルタを弾くためのもの。
            if old_content == new_content {
                continue;
            }

            // similar を使って行単位の diff をコンテキスト付きで計算する。
            let text_diff = TextDiff::from_lines(&old_content, &new_content);

            // 関数コンテキストの抽出を準備する。
            let ext = Path::new(&path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            let func_pattern = Self::func_pattern_for_ext(ext);
            let old_lines: Vec<&str> = old_content.lines().collect();

            let context_radius = 3;
            let mut hunks = Vec::new();
            let mut total_added = 0usize;
            let mut total_deleted = 0usize;

            for group in text_diff.grouped_ops(context_radius) {
                let mut hunk_lines = Vec::new();

                for op in &group {
                    if word_diff {
                        for inline_change in text_diff.iter_inline_changes(op) {
                            let tag = match inline_change.tag() {
                                ChangeTag::Equal => DiffLineTag::Equal,
                                ChangeTag::Insert => {
                                    total_added += 1;
                                    DiffLineTag::Insert
                                }
                                ChangeTag::Delete => {
                                    total_deleted += 1;
                                    DiffLineTag::Delete
                                }
                            };

                            let old_line_no = inline_change.old_index().map(|i| i + 1);
                            let new_line_no = inline_change.new_index().map(|i| i + 1);

                            let segments: Vec<InlineSegment> = inline_change
                                .iter_strings_lossy()
                                .map(|(emphasized, value)| InlineSegment {
                                    text: value.into_owned(),
                                    emphasized,
                                })
                                .collect();

                            // セグメントのテキストを連結して content を組み立てる。
                            let content: String = segments
                                .iter()
                                .map(|s| s.text.trim_end_matches('\n').trim_end_matches('\r'))
                                .collect::<Vec<_>>()
                                .join("");
                            let content = Self::expand_tabs(&content, tab_width);

                            let has_emphasis = segments.iter().any(|s| s.emphasized);
                            let inline_segments = if has_emphasis { segments } else { Vec::new() };

                            hunk_lines.push(DiffLine {
                                tag,
                                old_line_no,
                                new_line_no,
                                inline_segments,
                                content,
                            });
                        }
                    } else {
                        for change in text_diff.iter_changes(op) {
                            let tag = match change.tag() {
                                ChangeTag::Equal => DiffLineTag::Equal,
                                ChangeTag::Insert => {
                                    total_added += 1;
                                    DiffLineTag::Insert
                                }
                                ChangeTag::Delete => {
                                    total_deleted += 1;
                                    DiffLineTag::Delete
                                }
                            };

                            let old_line_no = change.old_index().map(|i| i + 1);
                            let new_line_no = change.new_index().map(|i| i + 1);

                            let raw = change.value().trim_end_matches('\n').trim_end_matches('\r');
                            let content = Self::expand_tabs(raw, tab_width);

                            hunk_lines.push(DiffLine {
                                tag,
                                old_line_no,
                                new_line_no,
                                inline_segments: Vec::new(),
                                content,
                            });
                        }
                    }
                }

                // このハンクの関数コンテキストヘッダーを抽出する。
                let func_header = func_pattern.as_ref().and_then(|pat| {
                    // ハンク内で最初の行番号を求める(旧側)。
                    let first_old_line = hunk_lines.iter().find_map(|l| l.old_line_no);
                    let first_new_line = hunk_lines.iter().find_map(|l| l.new_line_no);
                    let start = first_old_line.or(first_new_line).unwrap_or(1);
                    if start > 0 && !old_lines.is_empty() {
                        let search_from = (start - 1).min(old_lines.len() - 1);
                        Self::find_func_header(&old_lines, search_from, pat)
                    } else {
                        None
                    }
                });

                hunks.push(DiffHunk {
                    lines: hunk_lines,
                    func_header,
                });
            }

            file_diffs.push(FileDiff {
                path,
                added_lines: total_added,
                deleted_lines: total_deleted,
                hunks,
            });
        }

        Ok(file_diffs)
    }

    /// 大文字小文字だけ異なるリネームのペア(パスが大文字小文字のみ異なり、
    /// blob の内容が同一な削除+追加)を構成するデルタのインデックスを見つける。
    ///
    /// diff 処理でスキップすべきインデックスの集合を返す。
    fn find_case_only_rename_indices(diff: &git2::Diff<'_>) -> std::collections::HashSet<usize> {
        use std::collections::HashMap;

        let mut skip = std::collections::HashSet::new();

        // 削除エントリを集める: 小文字化したパス → (インデックス, blob oid)。
        let mut deleted: HashMap<String, Vec<(usize, git2::Oid)>> = HashMap::new();
        // 追加エントリを集める: 小文字化したパス → (インデックス, blob oid)。
        let mut added: HashMap<String, Vec<(usize, git2::Oid)>> = HashMap::new();

        for (idx, delta) in diff.deltas().enumerate() {
            let status = delta.status();
            match status {
                git2::Delta::Deleted => {
                    if let Some(p) = delta.old_file().path() {
                        let key = p.to_string_lossy().to_lowercase();
                        let oid = delta.old_file().id();
                        deleted.entry(key).or_default().push((idx, oid));
                    }
                }
                git2::Delta::Added | git2::Delta::Untracked => {
                    if let Some(p) = delta.new_file().path() {
                        let key = p.to_string_lossy().to_lowercase();
                        let oid = delta.new_file().id();
                        added.entry(key).or_default().push((idx, oid));
                    }
                }
                _ => {}
            }
        }

        // ペアを照合する: 小文字化したパスと blob OID が同じで、実際のパスが異なるもの。
        for (lower_path, del_entries) in &deleted {
            if let Some(add_entries) = added.get(lower_path) {
                for &(del_idx, del_oid) in del_entries {
                    for &(add_idx, add_oid) in add_entries {
                        if !del_oid.is_zero() && del_oid == add_oid {
                            // 実際のパスが(完全に同一ではなく)異なることを確認する。
                            let del_delta = diff.get_delta(del_idx).unwrap();
                            let add_delta = diff.get_delta(add_idx).unwrap();
                            let del_path = del_delta.old_file().path().unwrap();
                            let add_path = add_delta.new_file().path().unwrap();
                            if del_path != add_path {
                                skip.insert(del_idx);
                                skip.insert(add_idx);
                            }
                        }
                    }
                }
            }
        }

        skip
    }

    /// デルタが大文字小文字だけのリネームを表すかどうかを判定する。すなわち
    /// old_path と new_path が大文字小文字を無視すれば一致するが、実際のバイト列は
    /// 異なる場合。どちらかのパスが存在しなければ false を返す。
    fn is_case_only_rename(delta: &git2::DiffDelta<'_>) -> bool {
        if let (Some(old_path), Some(new_path)) = (delta.old_file().path(), delta.new_file().path())
        {
            let old_s = old_path.to_string_lossy();
            let new_s = new_path.to_string_lossy();
            old_s != new_s && old_s.eq_ignore_ascii_case(&new_s)
        } else {
            false
        }
    }

    /// diff のファイルエントリから blob の内容を読む。blob が存在しない場合
    /// (新規または削除されたファイル)は空文字列を返す。
    fn blob_content(repo: &Repository, file: &git2::DiffFile<'_>) -> String {
        if file.id().is_zero() {
            return String::new();
        }
        match repo.find_blob(file.id()) {
            Ok(blob) => {
                // UTF-8 として試み、失敗したら非可逆変換にフォールバックする。
                String::from_utf8(blob.content().to_vec())
                    .unwrap_or_else(|_| String::from_utf8_lossy(blob.content()).to_string())
            }
            Err(_) => String::new(),
        }
    }
}
