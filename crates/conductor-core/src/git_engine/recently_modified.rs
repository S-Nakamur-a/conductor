//! worktree で最近触られたファイルの一覧。

use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result};
use git2::{Repository, StatusOptions, StatusShow};

/// worktree ルートからの相対パスを、dirty なファイル、直近 10 コミットで変更された
/// ファイルの順に、重複なく最大 limit 件返す。
pub fn recently_modified_files(worktree_path: &Path, limit: usize) -> Result<Vec<String>> {
    let repo = Repository::discover(worktree_path)
        .with_context(|| format!("failed to discover repo from {}", worktree_path.display()))?;

    let mut seen = HashSet::new();
    let mut result = Vec::new();
    let mut push = |path: String, result: &mut Vec<String>| {
        if result.len() < limit && seen.insert(path.clone()) {
            result.push(path);
        }
    };

    let mut opts = StatusOptions::new();
    opts.show(StatusShow::IndexAndWorkdir)
        .include_untracked(true);
    if let Ok(statuses) = repo.statuses(Some(&mut opts)) {
        for entry in statuses.iter() {
            if let Some(path) = entry.path() {
                push(path.to_string(), &mut result);
            }
        }
    }

    if result.len() >= limit {
        return Ok(result);
    }
    let Ok(head_oid) = repo.head().and_then(|h| h.peel_to_commit()).map(|c| c.id()) else {
        return Ok(result);
    };
    let mut revwalk = repo.revwalk()?;
    revwalk.push(head_oid)?;
    revwalk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)?;
    for oid in revwalk.take(10) {
        if result.len() >= limit {
            break;
        }
        let Ok(commit) = oid.and_then(|oid| repo.find_commit(oid)) else {
            continue;
        };
        let Ok(tree) = commit.tree() else {
            continue;
        };
        let parent_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());
        let Ok(diff) = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None) else {
            continue;
        };
        for delta in diff.deltas() {
            if let Some(path) = delta.new_file().path() {
                push(path.to_string_lossy().into_owned(), &mut result);
            }
        }
    }
    Ok(result)
}
