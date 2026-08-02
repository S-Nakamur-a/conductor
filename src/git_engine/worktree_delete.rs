//! ブランチの削除、worktree の削除、古くなった worktree エントリの整理。

use std::path::Path;

use anyhow::{Context, Result};

use super::GitEngine;

impl GitEngine {
    // 強制削除 (wt rm -b -f)

    /// 指定した名前のローカルブランチを削除する。force が true の場合は
    /// -D 相当で、fully merged でなくても削除する。
    pub fn delete_branch(&self, name: &str, force: bool) -> Result<()> {
        let mut branch = self
            .repo
            .find_branch(name, git2::BranchType::Local)
            .with_context(|| format!("branch '{name}' not found"))?;
        if force {
            // 強制削除: reference を直接消すだけ。
            let ref_name = format!("refs/heads/{name}");
            if let Ok(mut reference) = self.repo.find_reference(&ref_name) {
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

    /// branch 上の全コミットがすでに into に含まれている(tip が一致する
    /// か into の tip の祖先である)場合に true を返す。worktree の削除が
    /// ブランチを強制削除してコミットが到達不能になる前に警告するために使う
    /// — libgit2 の非強制の Branch::delete は git CLI がやる
    /// "not fully merged" 拒否を行わないので、このチェックが唯一の防御線。
    pub fn is_branch_merged_into(&self, branch: &str, into: &str) -> Result<bool> {
        let branch_oid = self
            .repo
            .find_branch(branch, git2::BranchType::Local)
            .with_context(|| format!("branch '{branch}' not found"))?
            .get()
            .peel_to_commit()?
            .id();
        let into_oid = self
            .repo
            .find_branch(into, git2::BranchType::Local)
            .with_context(|| format!("branch '{into}' not found"))?
            .get()
            .peel_to_commit()?
            .id();
        if branch_oid == into_oid {
            return Ok(true);
        }
        Ok(self.repo.graph_descendant_of(into_oid, branch_oid)?)
    }

    // 古い worktree の整理 (wt prune)

    /// ディレクトリがすでに存在しない(stale な) worktree エントリを探す。
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

    /// stale な worktree エントリを1つ整理する。
    pub fn prune_stale_worktree(&self, name: &str) -> Result<()> {
        let wt = self
            .repo
            .find_worktree(name)
            .with_context(|| format!("worktree '{name}' not found"))?;

        wt.prune(Some(git2::WorktreePruneOptions::new().working_tree(true)))
            .with_context(|| format!("failed to prune stale worktree '{name}'"))?;

        Ok(())
    }

    /// 名前を指定して linked worktree を削除する。
    ///
    /// worktree エントリを prune し、必要ならディレクトリも削除する。
    /// main worktree は削除できない。
    pub fn remove_worktree(&self, worktree_path: &Path) -> Result<()> {
        let name = self
            .find_worktree_name_by_path(worktree_path)
            .with_context(|| format!("no worktree found for path {}", worktree_path.display()))?;
        let wt = self
            .repo
            .find_worktree(&name)
            .with_context(|| format!("worktree '{name}' not found"))?;

        let wt_path = wt.path().to_path_buf();

        // まず有効性を確認する
        if wt.validate().is_ok() {
            // worktree は有効で存在している — prune する
            wt.prune(Some(
                git2::WorktreePruneOptions::new()
                    .working_tree(true)
                    .valid(true),
            ))
            .with_context(|| format!("failed to prune worktree '{name}'"))?;
        } else {
            // worktree はすでに無効(ディレクトリが削除済みなど) — prune するだけ
            wt.prune(Some(git2::WorktreePruneOptions::new().working_tree(true)))
                .with_context(|| format!("failed to prune worktree '{name}'"))?;
        }

        // ディレクトリがまだ存在すれば削除する
        if wt_path.exists() {
            std::fs::remove_dir_all(&wt_path).with_context(|| {
                format!("failed to remove worktree directory {}", wt_path.display())
            })?;
        }

        Ok(())
    }

    /// 指定したパスに対応する libgit2 の worktree 名を探す。
    ///
    /// worktree 名はブランチ名と異なる場合がある(例えば feature/foo から
    /// foo という名前の worktree が作られる)ので、登録済みの全 worktree を
    /// 走査してパスで照合する。
    fn find_worktree_name_by_path(&self, target: &Path) -> Option<String> {
        let names = self.repo.worktrees().ok()?;
        for name in names.iter().flatten() {
            if let Ok(wt) = self.repo.find_worktree(name)
                && wt.path() == target
            {
                return Some(name.to_string());
            }
        }
        None
    }
}
