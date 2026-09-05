//! 他のブランチのコミットを 1 つ、この worktree へ cherry-pick する処理。

use std::path::Path;

use anyhow::{Context, Result};
use git2::Repository;

use super::GitEngine;
use super::commit_log::first_line;
use crate::diff_state::short_oid;

impl GitEngine {
    /// commit_oid_str を worktree_path のリポジトリへ cherry-pick し、結果の説明文を返す。
    /// コンフリクトしたら HEAD に戻して中止し、それも説明文として返す (Err ではない)。
    pub fn cherry_pick_to_worktree(
        &self,
        worktree_path: &Path,
        commit_oid_str: &str,
    ) -> Result<String> {
        let repo = Repository::open(worktree_path)
            .with_context(|| format!("cannot open worktree repo at {}", worktree_path.display()))?;
        let oid = git2::Oid::from_str(commit_oid_str)
            .with_context(|| format!("invalid OID: {commit_oid_str}"))?;
        let commit = repo
            .find_commit(oid)
            .with_context(|| format!("commit {commit_oid_str} not found"))?;

        repo.cherrypick(&commit, None)
            .with_context(|| format!("cherry-pick failed for {commit_oid_str}"))?;

        let mut index = repo.index()?;
        if index.has_conflicts() {
            repo.cleanup_state()?;
            let head = repo.head()?.peel_to_commit()?;
            repo.reset(head.as_object(), git2::ResetType::Hard, None)?;
            return Ok(format!(
                "Cherry-pick of {} aborted due to conflicts.",
                short_oid(commit_oid_str)
            ));
        }

        let tree = repo.find_tree(index.write_tree()?)?;
        let head_commit = repo.head()?.peel_to_commit()?;
        let sig = repo
            .signature()
            .or_else(|_| git2::Signature::now("Conductor", "conductor@localhost"))
            .context("cannot create signature")?;
        repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            commit.message().unwrap_or("cherry-picked commit"),
            &tree,
            &[&head_commit],
        )?;
        repo.cleanup_state()?;

        Ok(format!(
            "Cherry-picked {} \"{}\" successfully.",
            short_oid(commit_oid_str),
            first_line(commit.message())
        ))
    }
}
