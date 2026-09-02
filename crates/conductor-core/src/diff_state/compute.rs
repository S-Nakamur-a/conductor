//! git2 で diff を取り、ファイル単位の [FileDiff] に展開する。

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;

use anyhow::{Context, Result};
use git2::{Delta, DiffFile, Oid, Repository};
use regex::Regex;
use similar::{ChangeTag, TextDiff};

use super::{DiffHunk, DiffLine, DiffLineTag, DiffState, FileDiff, InlineSegment};

const CONTEXT_LINES: u32 = 3;
const FUNC_HEADER_MAX_BYTES: usize = 80;

impl DiffState {
    /// worktree_path のリポジトリについてベースからの変更を読み直す。
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
    }

    /// merge-base(base, HEAD) から作業ツリー (index 込み) までのファイル単位 diff をパス順に返す。
    ///
    /// 2 つ目はベースを解決できず HEAD 基準に落ちた理由。ベース設定のミスで手元の
    /// 未コミット変更まで見えなくなってはいけないので、一覧は返した上で理由を別に渡す。
    pub fn compute_changed_files(
        worktree_path: &Path,
        base_branch: &str,
        word_diff: bool,
        tab_width: usize,
    ) -> Result<(Vec<FileDiff>, Option<String>)> {
        let repo = Repository::open(worktree_path)
            .with_context(|| format!("cannot open repo at {}", worktree_path.display()))?;
        let head_commit = repo
            .head()
            .context("cannot resolve HEAD")?
            .peel_to_commit()
            .context("cannot peel HEAD to commit")?;

        let (base_tree, base_error) = match merge_base_tree(&repo, base_branch, head_commit.id()) {
            Ok(tree) => (tree, None),
            Err(e) => (head_commit.tree()?, Some(format!("{e:#}"))),
        };

        let mut opts = git2::DiffOptions::new();
        opts.include_untracked(true);
        opts.recurse_untracked_dirs(true);
        // これが無いと未追跡ファイルは一覧に出るだけで中身が読まれず、追加行数が 0 になる。
        opts.show_untracked_content(true);
        opts.context_lines(CONTEXT_LINES);
        let diff = repo.diff_tree_to_workdir_with_index(Some(&base_tree), Some(&mut opts))?;

        let skip = case_only_rename_indices(&diff)
            .union(&deletions_still_on_disk(&repo, &diff))
            .copied()
            .collect::<HashSet<_>>();
        let mut files = Vec::new();
        for idx in (0..diff.deltas().len()).filter(|idx| !skip.contains(idx)) {
            if let Some(file) = file_diff(&repo, &diff, idx, word_diff, tab_width)? {
                files.push(file);
            }
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok((files, base_error))
    }
}

fn merge_base_tree<'r>(repo: &'r Repository, base: &str, head: Oid) -> Result<git2::Tree<'r>> {
    let base_oid = resolve_base_commit(repo, base)?;
    let merge_base = repo
        .merge_base(base_oid, head)
        .with_context(|| format!("cannot find merge-base between '{base}' and HEAD"))?;
    repo.find_commit(merge_base)?
        .tree()
        .context("cannot get merge-base tree")
}

/// revparse なので git と同じく タグ > ブランチ の順で解決し、リモート追跡 ref も通る。
/// 裸の名前 (develop) が refs/remotes/origin/develop にしか無いことがあるので origin/ を
/// 補って再試行するが、エラーには利用者が設定した綴りの方を残す。
fn resolve_base_commit(repo: &Repository, base: &str) -> Result<Oid> {
    let resolved = repo.revparse_single(base).or_else(|primary| {
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

/// 内容に変化の無いデルタ (大文字小文字を区別しない FS の stat 不一致で出る) は None。
/// バイナリは Patch が Some でハンク 0 なので、行数 0 のまま一覧に残す。
fn file_diff(
    repo: &Repository,
    diff: &git2::Diff<'_>,
    idx: usize,
    word_diff: bool,
    tab_width: usize,
) -> Result<Option<FileDiff>> {
    let delta = diff.get_delta(idx).unwrap();
    let path = delta
        .new_file()
        .path()
        .or_else(|| delta.old_file().path())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "(unknown)".to_string());

    let patch = match git2::Patch::from_diff(diff, idx) {
        Ok(Some(patch)) => patch,
        Ok(None) => return Ok(None),
        Err(e) => {
            log::warn!("diff: cannot build a patch for {path}: {e}");
            return Ok(None);
        }
    };
    let (_, added_lines, deleted_lines) = patch.line_stats()?;
    let is_binary = delta.new_file().is_binary() || delta.old_file().is_binary();
    if added_lines == 0 && deleted_lines == 0 && !is_binary {
        return Ok(None);
    }

    let func_pattern = func_pattern_for(&path);
    let old_content = blob_content(repo, &delta.old_file());
    let old_lines: Vec<&str> = old_content.lines().collect();

    let mut hunks = Vec::with_capacity(patch.num_hunks());
    for hunk_idx in 0..patch.num_hunks() {
        let mut lines = Vec::new();
        for line_idx in 0..patch.num_lines_in_hunk(hunk_idx)? {
            let line = patch.line_in_hunk(hunk_idx, line_idx)?;
            // '=' '>' '<' は「末尾に改行が無い」の注記行で本文ではない。
            let tag = match line.origin() {
                '+' => DiffLineTag::Insert,
                '-' => DiffLineTag::Delete,
                ' ' => DiffLineTag::Equal,
                _ => continue,
            };
            let raw = String::from_utf8_lossy(line.content());
            let raw = raw.trim_end_matches('\n').trim_end_matches('\r');
            lines.push(DiffLine {
                tag,
                old_line_no: line.old_lineno().map(|n| n as usize),
                new_line_no: line.new_lineno().map(|n| n as usize),
                inline_segments: Vec::new(),
                content: expand_tabs(raw, tab_width),
            });
        }
        if word_diff {
            attach_inline_segments(&mut lines);
        }
        let func_header = func_pattern
            .and_then(|pattern| func_header_above(&old_lines, hunk_start(&lines), pattern));
        hunks.push(DiffHunk { lines, func_header });
    }

    Ok(Some(FileDiff {
        path,
        added_lines,
        deleted_lines,
        hunks,
    }))
}

fn hunk_start(lines: &[DiffLine]) -> usize {
    lines
        .iter()
        .find_map(|l| l.old_line_no)
        .or_else(|| lines.iter().find_map(|l| l.new_line_no))
        .unwrap_or(1)
}

static FUNC_PATTERNS: LazyLock<Vec<(&[&str], Regex)>> = LazyLock::new(|| {
    let table: &[(&[&str], &str)] = &[
        (
            &["rs"],
            r"^\s*(pub\s+)?(async\s+)?(fn|impl|struct|enum|trait|mod|macro_rules!)\b",
        ),
        (
            &["ts", "tsx", "js", "jsx", "mjs", "mts", "cjs", "cts"],
            r"^\s*(export\s+)?(default\s+)?(async\s+)?(function\*?|class)\b|^\s*(export\s+)?(const|let|var)\s+\w+\s*=",
        ),
        (&["py"], r"^\s*(async\s+)?(def|class)\b"),
        (&["go"], r"^(func|type)\b"),
        (
            &["java", "cs", "kt", "kts"],
            r"^\s*(public|private|protected|internal|static|abstract|override|final|suspend)?\s*(public|private|protected|internal|static|abstract|override|final|suspend)?\s*(class|interface|enum|record|fun|void|int|long|string|bool|boolean|var|val|object)\b",
        ),
        (
            &["c", "h", "cpp", "cc", "cxx", "hpp", "hxx"],
            r"^[a-zA-Z_][\w:*&<> ]*\s+\*?\w+\s*\(|^\s*(class|struct|enum|namespace|template)\b",
        ),
        (&["rb"], r"^\s*(def|class|module)\b"),
        (
            &["php"],
            r"^\s*(public|private|protected|static)?\s*(function|class|interface|trait)\b",
        ),
        (&["sh", "bash", "zsh"], r"^\s*(\w+\s*\(\)|function\s+\w+)"),
    ];
    table
        .iter()
        .map(|(exts, pattern)| (*exts, Regex::new(pattern).unwrap()))
        .collect()
});

fn func_pattern_for(path: &str) -> Option<&'static Regex> {
    let ext = Path::new(path).extension()?.to_str()?;
    FUNC_PATTERNS
        .iter()
        .find(|(exts, _)| exts.contains(&ext))
        .map(|(_, re)| re)
}

/// 旧ファイルの start_line (1 始まり) から上に向かって最も近い関数ヘッダーを探す。
fn func_header_above(old_lines: &[&str], start_line: usize, pattern: &Regex) -> Option<String> {
    let from = start_line.min(old_lines.len());
    old_lines[..from]
        .iter()
        .rev()
        .map(|line| line.trim_end())
        .find(|line| pattern.is_match(line))
        .map(|line| truncate_header(line.trim()))
}

fn truncate_header(line: &str) -> String {
    if line.len() <= FUNC_HEADER_MAX_BYTES {
        return line.to_string();
    }
    format!(
        "{}…",
        &line[..line.floor_char_boundary(FUNC_HEADER_MAX_BYTES)]
    )
}

/// libgit2 は行単位までしか出さないので、連続する削除と追加を並び順で対応付けて
/// 単語 diff にかける。対応の無い行は空のままにして行全体を描かせる。
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
        let ins_start = i;
        while i < lines.len() && lines[i].tag == DiffLineTag::Insert {
            i += 1;
        }
        let pairs = (ins_start - del_start).min(i - ins_start);
        for k in 0..pairs {
            let (del, ins) =
                word_segments(&lines[del_start + k].content, &lines[ins_start + k].content);
            lines[del_start + k].inline_segments = del;
            lines[ins_start + k].inline_segments = ins;
        }
    }
}

/// (削除側, 追加側) の分割。強調される片が無ければ空。
fn word_segments(old: &str, new: &str) -> (Vec<InlineSegment>, Vec<InlineSegment>) {
    let words = TextDiff::from_words(old, new);
    let mut del = Vec::new();
    let mut ins = Vec::new();
    for change in words.iter_all_changes() {
        let segment = |emphasized| InlineSegment {
            text: change.value().to_string(),
            emphasized,
        };
        match change.tag() {
            ChangeTag::Equal => {
                del.push(segment(false));
                ins.push(segment(false));
            }
            ChangeTag::Delete => del.push(segment(true)),
            ChangeTag::Insert => ins.push(segment(true)),
        }
    }
    let only_if_emphasized = |segments: Vec<InlineSegment>| {
        if segments.iter().any(|s| s.emphasized) {
            segments
        } else {
            Vec::new()
        }
    };
    (only_if_emphasized(del), only_if_emphasized(ins))
}

/// ケース違いの 2 エントリに実ファイルが 1 つしか無いと、libgit2 は余った方を削除と
/// 報告する (git 本体は clean)。DiffOptions では直らない。
fn deletions_still_on_disk(repo: &Repository, diff: &git2::Diff<'_>) -> HashSet<usize> {
    let Some(workdir) = repo.workdir() else {
        return HashSet::new();
    };
    diff.deltas()
        .enumerate()
        .filter(|(_, delta)| delta.status() == Delta::Deleted)
        .filter_map(|(idx, delta)| {
            let path = delta.old_file().path()?;
            workdir.join(path).is_file().then_some(idx)
        })
        .collect()
}

/// パスが大文字小文字だけ違い blob が同一な削除と追加の組。大文字小文字を区別しない
/// FS では同じファイルなので、変更として出さない。
fn case_only_rename_indices(diff: &git2::Diff<'_>) -> HashSet<usize> {
    let lower = |file: &DiffFile<'_>| Some(file.path()?.to_string_lossy().to_lowercase());
    let mut deleted: HashMap<String, Vec<(usize, Oid)>> = HashMap::new();
    let mut added: HashMap<String, Vec<(usize, Oid)>> = HashMap::new();
    for (idx, delta) in diff.deltas().enumerate() {
        match delta.status() {
            Delta::Deleted => {
                if let Some(key) = lower(&delta.old_file()) {
                    deleted
                        .entry(key)
                        .or_default()
                        .push((idx, delta.old_file().id()));
                }
            }
            Delta::Added | Delta::Untracked => {
                if let Some(key) = lower(&delta.new_file()) {
                    added
                        .entry(key)
                        .or_default()
                        .push((idx, delta.new_file().id()));
                }
            }
            _ => {}
        }
    }

    let mut skip = HashSet::new();
    for (key, dels) in &deleted {
        let Some(adds) = added.get(key) else {
            continue;
        };
        for &(del_idx, del_oid) in dels {
            for &(add_idx, add_oid) in adds {
                if !del_oid.is_zero() && del_oid == add_oid {
                    skip.insert(del_idx);
                    skip.insert(add_idx);
                }
            }
        }
    }
    skip
}

fn blob_content(repo: &Repository, file: &DiffFile<'_>) -> String {
    if file.id().is_zero() {
        return String::new();
    }
    repo.find_blob(file.id())
        .map(|blob| String::from_utf8_lossy(blob.content()).into_owned())
        .unwrap_or_default()
}

fn expand_tabs(line: &str, tab_width: usize) -> String {
    if !line.contains('\t') {
        return line.to_string();
    }
    let mut result = String::with_capacity(line.len());
    let mut col = 0;
    for ch in line.chars() {
        if ch == '\t' {
            let spaces = tab_width - (col % tab_width);
            result.extend(std::iter::repeat_n(' ', spaces));
            col += spaces;
        } else {
            result.push(ch);
            col += 1;
        }
    }
    result
}
