//! git2/similar ベースの diff 計算。リポジトリからの DiffState の読み込み、
//! ファイル単位のハンク/行の構築、関数コンテキストヘッダーの検出、
//! 大文字小文字の違いだけのリネームのフィルタリング(大文字小文字を区別しない
//! ファイルシステム向け)を行う。

use std::path::Path;

use anyhow::{Context, Result};
use git2::Repository;
use regex::Regex;
use similar::{ChangeTag, TextDiff};

use super::model::{DiffHunk, DiffLine, DiffLineTag, DiffState, FileDiff, InlineSegment};

/// ハンクの前後に付けるコンテキスト行数。git の既定と同じ 3 行。
const CONTEXT_LINES: u32 = 3;

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
    /// worktree_path のリポジトリについてベースからの変更を読み込み、
    /// 以前に保持していた diff データを置き換える。
    pub fn load_diff(
        &mut self,
        worktree_path: &Path,
        base_branch: &str,
        word_diff: bool,
        tab_width: usize,
    ) {
        self.base_branch = base_branch.to_string();

        match Self::compute_changed_files(worktree_path, base_branch, word_diff, tab_width) {
            Ok((files, base_error)) => {
                self.files = files;
                self.error = base_error;
            }
            Err(e) => {
                self.files.clear();
                self.error = Some(format!("{e:#}"));
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

    /// merge-base(base, HEAD) から作業ツリー(index 込み)までのファイル単位 diff を
    /// パス順に並べて返す。コミット済みと未コミットを1本の diff にまとめているので、
    /// コミット後に再編集したファイルも1エントリのままになる。
    ///
    /// 2つ目の戻り値は「ベースを解決できず HEAD を基準にフォールバックした」理由。
    /// ベース解決の失敗で一覧ごと空にすると手元の未コミット変更まで見えなくなるので、
    /// 一覧は返した上で理由だけを別に渡す。
    pub fn compute_changed_files(
        worktree_path: &Path,
        base_branch: &str,
        word_diff: bool,
        tab_width: usize,
    ) -> Result<(Vec<FileDiff>, Option<String>)> {
        let repo = Repository::open(worktree_path)
            .with_context(|| format!("cannot open repo at {}", worktree_path.display()))?;

        // HEAD を解決する。
        let head_commit = repo
            .head()
            .with_context(|| "cannot resolve HEAD")?
            .peel_to_commit()
            .with_context(|| "cannot peel HEAD to commit")?;
        let head_tree = head_commit.tree()?;

        let (base_tree, base_error) =
            match Self::merge_base_tree(&repo, base_branch, head_commit.id()) {
                Ok(tree) => (tree, None),
                Err(e) => (head_tree, Some(format!("{e:#}"))),
            };

        let mut opts = git2::DiffOptions::new();
        opts.include_untracked(true);
        opts.recurse_untracked_dirs(true);
        // これが無いと未追跡ファイルは一覧に出るだけで中身が読まれず、
        // パッチが作られないので追加行数が 0 になってしまう。
        opts.show_untracked_content(true);
        opts.context_lines(CONTEXT_LINES);
        let diff = repo.diff_tree_to_workdir_with_index(Some(&base_tree), Some(&mut opts))?;
        let files = Self::file_diffs_from(&repo, &diff, word_diff, tab_width)?;
        Ok((files, base_error))
    }

    /// base と HEAD の merge-base のツリーを解決する。
    fn merge_base_tree<'r>(
        repo: &'r Repository,
        base_branch: &str,
        head_oid: git2::Oid,
    ) -> Result<git2::Tree<'r>> {
        let base_oid = resolve_base_commit(repo, base_branch)?;
        let merge_base_oid = repo
            .merge_base(base_oid, head_oid)
            .with_context(|| format!("cannot find merge-base between '{base_branch}' and HEAD"))?;
        repo.find_commit(merge_base_oid)?
            .tree()
            .with_context(|| "cannot get merge-base tree")
    }

    /// git2 の diff をファイル単位の [FileDiff] に展開する(行内 diff は
    /// word_diff のときだけ計算する)。返り値はパス順。
    fn file_diffs_from(
        repo: &Repository,
        diff: &git2::Diff<'_>,
        word_diff: bool,
        tab_width: usize,
    ) -> Result<Vec<FileDiff>> {
        let mut file_diffs = Vec::new();

        let mut skip_indices = Self::find_case_only_rename_indices(diff);
        skip_indices.extend(Self::find_deletions_still_on_disk(repo, diff));

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

            // 差分そのものを libgit2 に作らせる。ここが自前でファイルを読んで
            // 数え直していた箇所で、読めなかった内容が空文字列に化けて「全行削除」
            // として表示される事故の発生源だった。libgit2 に任せると内容の取得も
            // 行数も git 本体と同じ経路になり、.gitattributes のフィルタや改行
            // 正規化、バイナリ判定もそのまま効く。
            let patch = match git2::Patch::from_diff(diff, delta_idx) {
                Ok(p) => p,
                Err(e) => {
                    // 1ファイルの失敗で一覧全体を落とさない。数字を捏造するより
                    // 出さない方が害が小さく、次の再計算でやり直せる。
                    log::warn!("diff: cannot build a patch for {path}: {e}");
                    continue;
                }
            };

            // 内容に変化が無いデルタ。大文字小文字を区別しないファイルシステムの
            // stat 不一致で出る偽のデルタがここに来る。
            let Some(patch) = patch else {
                continue;
            };

            // バイナリ判定はパッチを作らせた後でないと確定しない(libgit2 が中身を
            // 見て初めてフラグを立てるため)。
            let is_binary = delta.new_file().is_binary() || delta.old_file().is_binary();

            let (_context, added_lines, deleted_lines) = patch.line_stats()?;

            if added_lines == 0 && deleted_lines == 0 {
                // バイナリは行数を出せないだけで変更自体はある。落とすと「変更した
                // のに一覧に出ない」ことになるので、行数なしの項目として残す。
                // numstat がバイナリを "-" と表示するのと同じ扱い。
                if is_binary {
                    file_diffs.push(FileDiff {
                        path,
                        added_lines: 0,
                        deleted_lines: 0,
                        hunks: Vec::new(),
                    });
                }
                continue;
            }

            // 関数コンテキストの抽出を準備する。旧内容は ODB の blob からしか
            // 読まないので、workdir の状態に左右されない。
            let ext = Path::new(&path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            let func_pattern = Self::func_pattern_for_ext(ext);
            let old_content = Self::blob_content(repo, &delta.old_file());
            let old_lines: Vec<&str> = old_content.lines().collect();

            let mut hunks = Vec::new();
            for hunk_idx in 0..patch.num_hunks() {
                let num_lines = patch.num_lines_in_hunk(hunk_idx)?;
                let mut hunk_lines = Vec::new();

                for line_idx in 0..num_lines {
                    let line = patch.line_in_hunk(hunk_idx, line_idx)?;
                    let tag = match line.origin() {
                        '+' => DiffLineTag::Insert,
                        '-' => DiffLineTag::Delete,
                        // ' ' はコンテキスト。'=' '>' '<' は「末尾に改行が無い」
                        // ことを示す注記行で、本文ではないので描画しない。
                        ' ' => DiffLineTag::Equal,
                        _ => continue,
                    };

                    let raw = String::from_utf8_lossy(line.content());
                    let raw = raw.trim_end_matches('\n').trim_end_matches('\r');

                    hunk_lines.push(DiffLine {
                        tag,
                        old_line_no: line.old_lineno().map(|n| n as usize),
                        new_line_no: line.new_lineno().map(|n| n as usize),
                        inline_segments: Vec::new(),
                        content: Self::expand_tabs(raw, tab_width),
                    });
                }

                if word_diff {
                    Self::attach_inline_segments(&mut hunk_lines);
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
                added_lines,
                deleted_lines,
                hunks,
            });
        }

        file_diffs.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(file_diffs)
    }

    /// word diff 用に、ハンク内の削除行と追加行を対にして行内の変更箇所を求める。
    ///
    /// libgit2 は行単位までしか出さないので、置き換えとみなせる削除ブロックと
    /// 追加ブロックを並び順で対応付け、対になった行同士を単語単位で diff する。
    /// 対応が付かない余りの行(片側だけ増減した分)は行全体をそのまま描画させる。
    fn attach_inline_segments(lines: &mut [DiffLine]) {
        let mut i = 0;
        while i < lines.len() {
            if lines[i].tag != DiffLineTag::Delete {
                i += 1;
                continue;
            }
            let del_start = i;
            while i < lines.len() && lines[i].tag == DiffLineTag::Delete {
                i += 1;
            }
            let del_end = i;
            let ins_start = i;
            while i < lines.len() && lines[i].tag == DiffLineTag::Insert {
                i += 1;
            }
            let ins_end = i;

            for k in 0..(del_end - del_start).min(ins_end - ins_start) {
                let old = lines[del_start + k].content.clone();
                let new = lines[ins_start + k].content.clone();
                let words = TextDiff::from_words(&old, &new);

                let mut del_segments = Vec::new();
                let mut ins_segments = Vec::new();
                for change in words.iter_all_changes() {
                    let text = change.value().to_string();
                    match change.tag() {
                        ChangeTag::Equal => {
                            del_segments.push(InlineSegment {
                                text: text.clone(),
                                emphasized: false,
                            });
                            ins_segments.push(InlineSegment {
                                text,
                                emphasized: false,
                            });
                        }
                        ChangeTag::Delete => del_segments.push(InlineSegment {
                            text,
                            emphasized: true,
                        }),
                        ChangeTag::Insert => ins_segments.push(InlineSegment {
                            text,
                            emphasized: true,
                        }),
                    }
                }

                // 強調箇所が無いなら空のままにして、行全体の描画に任せる。
                if del_segments.iter().any(|s| s.emphasized) {
                    lines[del_start + k].inline_segments = del_segments;
                }
                if ins_segments.iter().any(|s| s.emphasized) {
                    lines[ins_start + k].inline_segments = ins_segments;
                }
            }
        }
    }

    /// 大文字小文字を区別しない FS ではケース違いの2エントリに実ファイルが1つしか
    /// 無く、libgit2 は余った方を削除として報告する(git 本体は clean)。DiffOptions
    /// では直らない — ignore_case が変えるのはデルタの並び順だけ。
    fn find_deletions_still_on_disk(
        repo: &Repository,
        diff: &git2::Diff<'_>,
    ) -> std::collections::HashSet<usize> {
        let Some(workdir) = repo.workdir() else {
            return std::collections::HashSet::new();
        };
        diff.deltas()
            .enumerate()
            .filter(|(_, delta)| delta.status() == git2::Delta::Deleted)
            .filter_map(|(idx, delta)| {
                let path = delta.old_file().path()?;
                workdir.join(path).is_file().then_some(idx)
            })
            .collect()
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
