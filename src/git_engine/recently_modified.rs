//! worktree 内で最近触れられたファイルを見つける独立ヘルパー。

use std::path::Path;

use anyhow::{Context, Result};
use git2::{Repository, StatusOptions, StatusShow};

/// 最近変更されたファイルパス(worktree ルートからの相対パス)の一覧を返す。
///
/// git status の dirty なファイルと、直近のコミットで変更されたファイルを
/// 合わせて (limit 件の一意なパスまで) 収集する。他の箇所と同じパターンで
/// 自前でリポジトリを開く独立関数。
pub fn recently_modified_files(worktree_path: &Path, limit: usize) -> Result<Vec<String>> {
    let repo = Repository::discover(worktree_path)
        .with_context(|| format!("failed to discover repo from {}", worktree_path.display()))?;

    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();

    // 1. ワーキングツリーの状態から dirty なファイルを集める。
    let mut opts = StatusOptions::new();
    opts.show(StatusShow::IndexAndWorkdir)
        .include_untracked(true);
    if let Ok(statuses) = repo.statuses(Some(&mut opts)) {
        for entry in statuses.iter() {
            if result.len() >= limit {
                break;
            }
            if let Some(path) = entry.path()
                && seen.insert(path.to_string())
            {
                result.push(path.to_string());
            }
        }
    }

    // 2. 直近のコミット(最大10件)で変更されたファイル。
    if result.len() < limit
        && let Ok(head) = repo.head()
        && let Some(oid) = head.target()
        && let Ok(mut revwalk) = repo.revwalk()
    {
        let _ = revwalk.push(oid);
        revwalk
            .set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)
            .ok();

        let mut commit_count = 0;
        for rev_oid in revwalk {
            if commit_count >= 10 || result.len() >= limit {
                break;
            }
            let rev_oid = match rev_oid {
                Ok(o) => o,
                Err(_) => continue,
            };
            let commit = match repo.find_commit(rev_oid) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let tree = match commit.tree() {
                Ok(t) => t,
                Err(_) => continue,
            };

            // 第一親との差分を取る(ルートコミットなら空ツリーとの差分)。
            let parent_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());
            if let Ok(diff) = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None) {
                for delta in diff.deltas() {
                    if result.len() >= limit {
                        break;
                    }
                    if let Some(path) = delta.new_file().path() {
                        let s = path.to_string_lossy().to_string();
                        if seen.insert(s.clone()) {
                            result.push(s);
                        }
                    }
                }
            }
            commit_count += 1;
        }
    }

    Ok(result)
}
