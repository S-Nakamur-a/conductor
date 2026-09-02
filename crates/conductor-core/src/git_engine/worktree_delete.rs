//! ブランチと worktree の削除、消えた worktree エントリの整理。

use std::path::Path;

use anyhow::{Context, Result};

use super::GitEngine;

impl GitEngine {
    /// ローカルブランチを削除する。force は `git branch -D` に相当。
    pub fn delete_branch(&self, name: &str, force: bool) -> Result<()> {
        let mut branch = self
            .repo
            .find_branch(name, git2::BranchType::Local)
            .with_context(|| format!("branch '{name}' not found"))?;
        if force {
            if let Ok(mut reference) = self.repo.find_reference(&format!("refs/heads/{name}")) {
                reference
                    .delete()
                    .with_context(|| format!("failed to force-delete branch '{name}'"))?;
            }
        } else {
            branch
                .delete()
                .with_context(|| format!("failed to delete branch '{name}' (not fully merged?)"))?;
        }
        Ok(())
    }

    /// branch の tip が into の tip と一致するか、その祖先なら true。
    ///
    /// libgit2 の Branch::delete は git CLI の "not fully merged" 拒否をしないので、
    /// worktree の削除がコミットを到達不能にする前の唯一の防御線になる。
    pub fn is_branch_merged_into(&self, branch: &str, into: &str) -> Result<bool> {
        let branch_oid = self.local_branch_oid(branch)?;
        let into_oid = self.local_branch_oid(into)?;
        if branch_oid == into_oid {
            return Ok(true);
        }
        Ok(self.repo.graph_descendant_of(into_oid, branch_oid)?)
    }

    /// ディレクトリが既に無い worktree エントリの名前を返す。
    pub fn find_stale_worktrees(&self) -> Result<Vec<String>> {
        let mut stale = Vec::new();
        if let Ok(names) = self.repo.worktrees() {
            for name in names.iter().flatten() {
                if let Ok(wt) = self.repo.find_worktree(name)
                    && wt.validate().is_err()
                {
                    stale.push(name.to_string());
                }
            }
        }
        Ok(stale)
    }

    pub fn prune_stale_worktree(&self, name: &str) -> Result<()> {
        let wt = self
            .repo
            .find_worktree(name)
            .with_context(|| format!("worktree '{name}' not found"))?;
        wt.prune(Some(git2::WorktreePruneOptions::new().working_tree(true)))
            .with_context(|| format!("failed to prune stale worktree '{name}'"))?;
        Ok(())
    }

    /// linked worktree のエントリとディレクトリを削除する。main worktree は消せない。
    pub fn remove_worktree(&self, worktree_path: &Path) -> Result<()> {
        let name = self
            .find_worktree_name_by_path(worktree_path)
            .with_context(|| format!("no worktree found for path {}", worktree_path.display()))?;
        let wt = self
            .repo
            .find_worktree(&name)
            .with_context(|| format!("worktree '{name}' not found"))?;
        let wt_path = wt.path().to_path_buf();

        let mut prune_opts = git2::WorktreePruneOptions::new();
        prune_opts.working_tree(true);
        if wt.validate().is_ok() {
            prune_opts.valid(true);
        }
        wt.prune(Some(&mut prune_opts))
            .with_context(|| format!("failed to prune worktree '{name}'"))?;

        if wt_path.exists() {
            std::fs::remove_dir_all(&wt_path).with_context(|| {
                format!("failed to remove worktree directory {}", wt_path.display())
            })?;
        }
        Ok(())
    }

    fn local_branch_oid(&self, name: &str) -> Result<git2::Oid> {
        Ok(self
            .repo
            .find_branch(name, git2::BranchType::Local)
            .with_context(|| format!("branch '{name}' not found"))?
            .get()
            .peel_to_commit()?
            .id())
    }

    /// worktree 名はブランチ名と違うことがある (feature/foo から foo が作られる) ので、
    /// 登録済みを全部走査してパスで照合する。
    fn find_worktree_name_by_path(&self, target: &Path) -> Option<String> {
        let names = self.repo.worktrees().ok()?;
        names
            .iter()
            .flatten()
            .find(|name| {
                self.repo
                    .find_worktree(name)
                    .is_ok_and(|wt| wt.path() == target)
            })
            .map(String::from)
    }
}
