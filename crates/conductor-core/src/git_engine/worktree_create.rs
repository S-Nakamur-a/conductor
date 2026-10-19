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

    /// `origin/feature-x` のようなリモートブランチから worktree を作る。ローカルに
    /// 無ければ upstream 付きで新規ブランチを切り、同名があればリモートへ早送り
    /// してから checkout する。
    pub fn create_worktree_from_remote(
        &self,
        remote_branch: &str,
        worktree_dir_override: Option<&Path>,
    ) -> Result<PathBuf> {
        let local_branch = Self::strip_remote_prefix(remote_branch);
        if self.branch_checked_out_elsewhere(local_branch)? {
            anyhow::bail!("'{local_branch}' is already checked out in another worktree");
        }
        let dir_name = Self::strip_branch_prefix(local_branch);
        let wt_path = self
            .worktrees_base_dir(worktree_dir_override)?
            .join(dir_name);
        let existing = self
            .repo
            .find_branch(local_branch, git2::BranchType::Local)
            .is_ok();
        self.prepare_worktree_slot(&wt_path, dir_name)?;
        if existing {
            self.catch_up_with_remote(local_branch, remote_branch)?;
        }
        let path = wt_path.display().to_string();
        if existing {
            self.git_worktree_add(&[&path, local_branch])?;
        } else {
            self.git_worktree_add(&["--track", "-b", local_branch, &path, remote_branch])?;
        }
        Ok(wt_path)
    }

    /// ローカルブランチをリモートへ早送りする。分岐していれば ref は動かさない。
    ///
    /// worktree を作る前に済ませるのは、post-checkout フックがそのブランチに
    /// コミットを足すと、以後どう足掻いても早送りできなくなるため。
    fn catch_up_with_remote(&self, local_branch: &str, remote_branch: &str) -> Result<()> {
        let remote_oid = self
            .repo
            .find_reference(&format!("refs/remotes/{remote_branch}"))
            .with_context(|| format!("no such remote branch: {remote_branch}"))?
            .peel_to_commit()?
            .id();
        self.retarget_upstream(local_branch, remote_branch)?;
        let mut local = self
            .repo
            .find_reference(&format!("refs/heads/{local_branch}"))?;
        let local_oid = local.peel_to_commit()?.id();
        if remote_oid != local_oid && self.repo.graph_descendant_of(remote_oid, local_oid)? {
            local.set_target(remote_oid, "conductor: fast-forward to the remote branch")?;
        }
        Ok(())
    }

    /// upstream を remote_branch へ張り直す。既に別のリモートを追跡していれば
    /// 触らない。fork 構成で本家のブランチを覗いただけなのに、自分のフォークを
    /// 向いていた push 先が本家に差し替わるのを避ける。
    fn retarget_upstream(&self, local_branch: &str, remote_branch: &str) -> Result<()> {
        let mut branch = self
            .repo
            .find_branch(local_branch, git2::BranchType::Local)?;
        let remote = Self::remote_of(remote_branch);
        let other_remote = branch
            .upstream()
            .ok()
            .and_then(|up| up.name().ok().flatten().map(str::to_string))
            .is_some_and(|current| Self::remote_of(&current) != remote);
        if !other_remote {
            branch.set_upstream(Some(remote_branch))?;
        }
        Ok(())
    }

    /// local_branch をチェックアウト中の worktree が他にあるか。
    ///
    /// ディレクトリを手で消された登録も git は「使用中」と扱うので、開けるかどうかでは
    /// 落とさない。
    fn branch_checked_out_elsewhere(&self, local_branch: &str) -> Result<bool> {
        let target = format!("refs/heads/{local_branch}");
        let is_target = |repo: &git2::Repository| {
            repo.head()
                .ok()
                .and_then(|head| head.name().map(str::to_string))
                .is_some_and(|name| name == target)
        };
        if is_target(&self.repo) {
            return Ok(true);
        }
        if let Ok(main) = git2::Repository::open(self.main_worktree_path()?)
            && is_target(&main)
        {
            return Ok(true);
        }
        for name in self.repo.worktrees()?.iter().flatten() {
            if let Ok(wt) = self.repo.find_worktree(name)
                && git2::Repository::open_from_worktree(&wt).is_ok_and(|r| is_target(&r))
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// `origin/feature/x` のリモート名の部分。
    fn remote_of(remote_branch: &str) -> &str {
        remote_branch
            .split_once('/')
            .map_or(remote_branch, |(remote, _)| remote)
    }

    /// `origin/feature/x` から `feature/x` を取り出す。`origin/` だけを剥がすと
    /// `upstream/main` が丸ごとローカルブランチ名になり、`refs/heads/upstream/main`
    /// が `refs/remotes/upstream/main` を隠して自分自身を早送りする。
    fn strip_remote_prefix(remote_branch: &str) -> &str {
        remote_branch
            .split_once('/')
            .map_or(remote_branch, |(_, name)| name)
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

    /// このリポジトリの既定ブランチ。origin/HEAD の指す先、origin/main、
    /// origin/master、ローカルの main、master の順。
    pub fn default_branch(&self) -> String {
        if let Ok(head) = self.repo.find_reference("refs/remotes/origin/HEAD")
            && let Ok(resolved) = head.resolve()
            && let Some(name) = resolved
                .name()
                .and_then(|n| n.strip_prefix("refs/remotes/origin/"))
        {
            return name.to_string();
        }
        for cand in ["origin/main", "origin/master"] {
            if self.repo.revparse_single(cand).is_ok() {
                return cand.trim_start_matches("origin/").to_string();
            }
        }
        for cand in ["main", "master"] {
            if self.repo.find_branch(cand, git2::BranchType::Local).is_ok() {
                return cand.to_string();
            }
        }
        String::from("main")
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

    /// 置き場所が空いていることを確かめ、同じ置き場所を指す worktree エントリが
    /// 残っていれば消す。
    ///
    /// 名前だけで消さないのは、strip_branch_prefix が feature/topic と topic を
    /// 同じ名前に潰すため。別の場所で生きている worktree を未コミットごと消す。
    fn prepare_worktree_slot(&self, wt_path: &Path, name: &str) -> Result<()> {
        if wt_path.exists() {
            anyhow::bail!("directory already exists: {}", wt_path.display());
        }
        if let Ok(wt) = self.repo.find_worktree(name)
            && wt.path() == wt_path
        {
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
