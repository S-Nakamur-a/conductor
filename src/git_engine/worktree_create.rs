//! worktree の作成(ベース ref、既存ブランチ、リモートブランチから)と、
//! ベース ref をそれなりに最新に保つこと。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};

use super::GitEngine;

impl GitEngine {
    /// ベース ref から分岐する新しい worktree を作成する (wt new に相当)。worktree は
    /// `<worktree_dir_override か既定のベース>/<dir_name>` に配置される。
    pub fn create_worktree_from_base(
        &self,
        branch_name: &str,
        base_ref: &str,
        worktree_dir_override: Option<&Path>,
    ) -> Result<PathBuf> {
        // ブランチ名に誤って origin/ prefix が付くのを防ぐ。
        if branch_name.starts_with("origin/") {
            anyhow::bail!(
                "Branch name starts with 'origin/'. Did you mean to use switch?\n\
                 Use the branch name without 'origin/' prefix."
            );
        }

        let dir_name = Self::strip_branch_prefix(branch_name);
        let base_dir = self.worktrees_base_dir(worktree_dir_override)?;
        let wt_path = base_dir.join(dir_name);

        if wt_path.exists() {
            anyhow::bail!("directory already exists: {}", wt_path.display());
        }

        // この名前の既存 worktree エントリがあれば強制的に整理する。
        self.force_prune_worktree_entry(dir_name);

        // git worktree add の CLI を使う (libgit2 の worktree API より信頼できる)。output() では
        // なく spawn()+wait() を使うのは、バックグラウンドプロセスを立ち上げる post-checkout
        // hook でブロックしないため — output() はパイプが EOF になるまで読み続ける。
        let main_dir = self.main_worktree_path()?;
        let mut child = std::process::Command::new("git")
            .args([
                "worktree",
                "add",
                "-b",
                branch_name,
                &wt_path.display().to_string(),
                base_ref,
            ])
            .current_dir(&main_dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context("failed to run `git worktree add`")?;
        let status = child
            .wait()
            .context("failed to wait for `git worktree add`")?;
        if !status.success() {
            // stderr を読んでも安全: git が失敗したなら post-checkout hook は
            // 走っておらず、パイプを保持し続けるバックグラウンドプロセスもない。
            let mut stderr_buf = String::new();
            if let Some(mut stderr) = child.stderr.take() {
                use std::io::Read;
                let _ = stderr.read_to_string(&mut stderr_buf);
            }
            anyhow::bail!("git worktree add failed: {}", stderr_buf.trim());
        }

        Ok(wt_path)
    }

    /// ローカルにすでに存在するブランチをチェックアウトする worktree を作成する (PR intake)。
    /// ブランチは事前の fetch_refspec で作成済みなので `-b` なしの単純な
    /// `git worktree add <path> <branch>` になる。wt_dir は作成するディレクトリのフルパス。
    pub fn create_worktree_for_existing_branch(
        &self,
        branch: &str,
        wt_dir: &Path,
    ) -> Result<PathBuf> {
        if wt_dir.exists() {
            anyhow::bail!("directory already exists: {}", wt_dir.display());
        }

        let name = wt_dir.file_name().ok_or_else(|| {
            anyhow!(
                "cannot determine worktree name from path {}",
                wt_dir.display()
            )
        })?;
        self.force_prune_worktree_entry(&name.to_string_lossy());

        // output() ではなく spawn()+wait() を使う理由は create_worktree_from_base() を参照。
        let main_dir = self.main_worktree_path()?;
        let mut child = std::process::Command::new("git")
            .args(["worktree", "add", &wt_dir.display().to_string(), branch])
            .current_dir(&main_dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context("failed to run `git worktree add`")?;
        let status = child
            .wait()
            .context("failed to wait for `git worktree add`")?;
        if !status.success() {
            let mut stderr_buf = String::new();
            if let Some(mut stderr) = child.stderr.take() {
                use std::io::Read;
                let _ = stderr.read_to_string(&mut stderr_buf);
            }
            anyhow::bail!("git worktree add failed: {}", stderr_buf.trim());
        }

        Ok(wt_dir.to_path_buf())
    }

    /// base_branch のローカルブランチが存在し、それなりに最新であることを
    /// 保証する。force-update やチェックアウトは一切行わない。
    ///
    /// - ローカルブランチがまだ無ければ、refs/heads/<base_branch> に直接
    ///   fetch する。
    /// - すでに存在する場合は、まずリモートの tip を scratch ref に fetch し
    ///   (そのブランチが別の worktree でチェックアウトされていると、
    ///   refs/heads/<base_branch> へ直接 fetch するのは git に拒否される)、
    ///   fetch した tip がローカルの真の子孫である場合にのみローカル
    ///   ブランチを fast-forward する。fast-forward できない場合(分岐して
    ///   いる、あるいはすでに追いついている)はそのまま手を付けない —
    ///   これは diff のベースとなるメタデータであってユーザが実際に作業中の
    ///   ブランチではないので、ローカル履歴を黙って破棄するのは誤った
    ///   失敗のしかたになる。
    pub fn ensure_base_ref_available(&self, base_branch: &str) -> Result<()> {
        if self
            .repo
            .find_branch(base_branch, git2::BranchType::Local)
            .is_err()
        {
            self.fetch_refspec(&format!("{base_branch}:refs/heads/{base_branch}"))?;
            return Ok(());
        }

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
            let mut branch_ref = self
                .repo
                .find_reference(&format!("refs/heads/{base_branch}"))?;
            branch_ref.set_target(
                fetched_oid,
                "conductor: fast-forward base branch for PR intake",
            )?;
        }

        if let Ok(mut scratch) = self.repo.find_reference(SCRATCH_REF) {
            let _ = scratch.delete();
        }

        Ok(())
    }

    // リモートブランチ操作(wt switch)

    /// リモートブランチ(refs/remotes/origin/*)を、HEAD を除いて一覧する。
    pub fn list_remote_branches(&self) -> Result<Vec<String>> {
        let mut names = Vec::new();
        let branches = self.repo.branches(Some(git2::BranchType::Remote))?;
        for branch in branches {
            let (branch, _) = branch?;
            if let Some(name) = branch.name()? {
                // origin/HEAD はスキップする。
                if name.ends_with("/HEAD") {
                    continue;
                }
                names.push(name.to_string());
            }
        }
        names.sort();
        Ok(names)
    }

    /// main_branch から分岐する新しい worktree のための、実在する起点 ref
    /// のうち最良のものを解決する。
    ///
    /// リモート追跡ブランチ origin/<main_branch> を優先し、無ければローカル
    /// ブランチ <main_branch>、それも無ければ HEAD にフォールバックする。
    /// 返される ref は必ず解決できることが保証されるので、remote の無い
    /// リポジトリでも git worktree add ... <ref> が「invalid reference」で
    /// 失敗することはない。
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

    /// リモートブランチから worktree を作成する(wt switch に相当)。
    ///
    /// remote_branch は "origin/feature-x" のような形式。
    /// worktree_dir_override は worktree 用のカスタムベースディレクトリ
    /// (config の general.worktree_dir から)を任意で指定する。
    /// ローカルの追跡ブランチを作成し、upstream を設定する。
    pub fn create_worktree_from_remote(
        &self,
        remote_branch: &str,
        worktree_dir_override: Option<&Path>,
    ) -> Result<PathBuf> {
        let local_branch = remote_branch
            .strip_prefix("origin/")
            .unwrap_or(remote_branch);

        let dir_name = Self::strip_branch_prefix(local_branch);
        let base_dir = self.worktrees_base_dir(worktree_dir_override)?;
        let wt_path = base_dir.join(dir_name);

        if wt_path.exists() {
            anyhow::bail!("directory already exists: {}", wt_path.display());
        }

        // この名前の既存 worktree エントリがあれば強制的に整理する。
        self.force_prune_worktree_entry(dir_name);

        // git worktree add の CLI を使う — libgit2 の worktree API は古い
        // ロックや index の問題など様々なエッジケースで失敗しうるより信頼できる。
        // output() ではなく spawn()+wait() を使う理由は create_worktree_from_base() を参照。
        let main_dir = self.main_worktree_path()?;
        let mut child = std::process::Command::new("git")
            .args([
                "worktree",
                "add",
                "--track",
                "-b",
                local_branch,
                &wt_path.display().to_string(),
                remote_branch,
            ])
            .current_dir(&main_dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context("failed to run `git worktree add`")?;
        let status = child
            .wait()
            .context("failed to wait for `git worktree add`")?;
        if !status.success() {
            let mut stderr_buf = String::new();
            if let Some(mut stderr) = child.stderr.take() {
                use std::io::Read;
                let _ = stderr.read_to_string(&mut stderr_buf);
            }
            anyhow::bail!("git worktree add failed: {}", stderr_buf.trim());
        }

        Ok(wt_path)
    }

    /// ベストエフォート。エントリが無いこともあるのでエラーは黙って捨てる。
    /// 新しい worktree を作る前の後始末に使う。
    fn force_prune_worktree_entry(&self, name: &str) {
        if let Ok(wt) = self.repo.find_worktree(name) {
            let _ = wt.prune(Some(
                git2::WorktreePruneOptions::new()
                    .valid(true)
                    .working_tree(true),
            ));
        }
    }
}
