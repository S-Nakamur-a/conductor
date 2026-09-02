//! worktree の作成 (ベース ref、既存ブランチ、リモートブランチから) と、
//! ベース ref を最新に保つこと。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};

use super::GitEngine;

impl GitEngine {
    /// base_ref から branch_name を切って worktree を作る。置き場所は
    /// `worktrees_base_dir(worktree_dir_override)` の下。
    pub fn create_worktree_from_base(
        &self,
        branch_name: &str,
        base_ref: &str,
        worktree_dir_override: Option<&Path>,
    ) -> Result<PathBuf> {
        if branch_name.starts_with("origin/") {
            anyhow::bail!(
                "Branch name starts with 'origin/'. Did you mean to use switch?\n\
                 Use the branch name without 'origin/' prefix."
            );
        }

        let dir_name = Self::strip_branch_prefix(branch_name);
        let wt_path = self
            .worktrees_base_dir(worktree_dir_override)?
            .join(dir_name);
        self.prepare_worktree_slot(&wt_path, dir_name)?;
        self.git_worktree_add(&["-b", branch_name, &wt_path.display().to_string(), base_ref])?;
        Ok(wt_path)
    }

    /// ローカルに既にあるブランチをチェックアウトする worktree を wt_dir に作る (PR intake)。
    pub fn create_worktree_for_existing_branch(
        &self,
        branch: &str,
        wt_dir: &Path,
    ) -> Result<PathBuf> {
        let name = wt_dir.file_name().ok_or_else(|| {
            anyhow!(
                "cannot determine worktree name from path {}",
                wt_dir.display()
            )
        })?;
        self.prepare_worktree_slot(wt_dir, &name.to_string_lossy())?;
        self.git_worktree_add(&[&wt_dir.display().to_string(), branch])?;
        Ok(wt_dir.to_path_buf())
    }

    /// `origin/feature-x` のようなリモートブランチから、upstream 付きのローカル
    /// ブランチと worktree を作る。
    pub fn create_worktree_from_remote(
        &self,
        remote_branch: &str,
        worktree_dir_override: Option<&Path>,
    ) -> Result<PathBuf> {
        let local_branch = remote_branch
            .strip_prefix("origin/")
            .unwrap_or(remote_branch);
        let dir_name = Self::strip_branch_prefix(local_branch);
        let wt_path = self
            .worktrees_base_dir(worktree_dir_override)?
            .join(dir_name);
        self.prepare_worktree_slot(&wt_path, dir_name)?;
        self.git_worktree_add(&[
            "--track",
            "-b",
            local_branch,
            &wt_path.display().to_string(),
            remote_branch,
        ])?;
        Ok(wt_path)
    }

    /// base_branch のローカルブランチを用意し、origin の tip の純粋な子孫なら
    /// fast-forward する。分岐していれば触らない。
    pub fn ensure_base_ref_available(&self, base_branch: &str) -> Result<()> {
        if self
            .repo
            .find_branch(base_branch, git2::BranchType::Local)
            .is_err()
        {
            return self.fetch_refspec(&format!("{base_branch}:refs/heads/{base_branch}"));
        }

        // 別の worktree でチェックアウト中のブランチへの直接 fetch は git に拒否される
        // ので、scratch ref に取ってから自分で ref を進める。
        const SCRATCH_REF: &str = "refs/conductor/pr-intake-base-update";
        self.fetch_refspec(&format!("{base_branch}:{SCRATCH_REF}"))?;

        let local_oid = self
            .repo
            .find_branch(base_branch, git2::BranchType::Local)?
            .get()
            .peel_to_commit()?
            .id();
        let fetched_oid = self
            .repo
            .find_reference(SCRATCH_REF)?
            .peel_to_commit()?
            .id();

        if fetched_oid != local_oid && self.repo.graph_descendant_of(fetched_oid, local_oid)? {
            self.repo
                .find_reference(&format!("refs/heads/{base_branch}"))?
                .set_target(
                    fetched_oid,
                    "conductor: fast-forward base branch for PR intake",
                )?;
        }

        if let Ok(mut scratch) = self.repo.find_reference(SCRATCH_REF) {
            let _ = scratch.delete();
        }
        Ok(())
    }

    /// origin/HEAD を除いたリモート追跡ブランチをソート済みで返す。
    pub fn list_remote_branches(&self) -> Result<Vec<String>> {
        let mut names = Vec::new();
        for branch in self.repo.branches(Some(git2::BranchType::Remote))? {
            let (branch, _) = branch?;
            if let Some(name) = branch.name()?
                && !name.ends_with("/HEAD")
            {
                names.push(name.to_string());
            }
        }
        names.sort();
        Ok(names)
    }

    /// 新しい worktree の起点として必ず解決できる ref を返す。
    /// origin/<main_branch>、<main_branch>、HEAD の順。
    pub fn resolve_base_ref(&self, main_branch: &str) -> String {
        let remote = format!("origin/{main_branch}");
        if self.repo.revparse_single(&remote).is_ok() {
            return remote;
        }
        if self.repo.revparse_single(main_branch).is_ok() {
            return main_branch.to_string();
        }
        String::from("HEAD")
    }

    /// 置き場所が空いていることを確かめ、同名の worktree エントリが残っていれば消す。
    fn prepare_worktree_slot(&self, wt_path: &Path, name: &str) -> Result<()> {
        if wt_path.exists() {
            anyhow::bail!("directory already exists: {}", wt_path.display());
        }
        if let Ok(wt) = self.repo.find_worktree(name) {
            let _ = wt.prune(Some(
                git2::WorktreePruneOptions::new()
                    .valid(true)
                    .working_tree(true),
            ));
        }
        Ok(())
    }

    /// libgit2 の worktree API は古いロックや index の残骸で失敗しやすいので git CLI に任せる。
    /// output() ではなく spawn+wait なのは、post-checkout hook がバックグラウンド
    /// プロセスを残すとパイプの EOF を待ち続けて固まるため。
    fn git_worktree_add(&self, args: &[&str]) -> Result<()> {
        let main_dir = self.main_worktree_path()?;
        let mut child = std::process::Command::new("git")
            .args(["worktree", "add"])
            .args(args)
            .current_dir(&main_dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context("failed to run `git worktree add`")?;
        let status = child
            .wait()
            .context("failed to wait for `git worktree add`")?;
        if status.success() {
            return Ok(());
        }
        let mut stderr_buf = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            use std::io::Read;
            let _ = stderr.read_to_string(&mut stderr_buf);
        }
        anyhow::bail!("git worktree add failed: {}", stderr_buf.trim());
    }
}
