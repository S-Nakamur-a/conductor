//! pull (fetch + fast-forward)、ブランチの main への fast-forward マージ、
//! main の origin へのハードリセット。

use std::path::Path;

use anyhow::{Context, Result};
use git2::Repository;

use super::GitEngine;

impl GitEngine {
    /// origin から fetch し、worktree_path のブランチを upstream へ fast-forward する。
    /// fast-forward できなければ何もせず、その旨の説明文を返す。
    ///
    /// ネットワーク I/O を伴うので UI スレッドから呼ばないこと。
    pub fn pull_worktree(&self, worktree_path: &Path) -> Result<String> {
        let (branch_name, upstream_name) = {
            let wt_repo = Repository::open(worktree_path)
                .with_context(|| format!("cannot open worktree at {}", worktree_path.display()))?;
            let head = wt_repo.head().context("cannot read HEAD")?;
            if !head.is_branch() {
                anyhow::bail!("Cannot pull: HEAD is detached");
            }
            let branch_name = head.shorthand().unwrap_or("unknown").to_string();
            let upstream = wt_repo
                .find_branch(&branch_name, git2::BranchType::Local)
                .with_context(|| format!("branch '{branch_name}' not found"))?
                .upstream()
                .with_context(|| format!("No upstream configured for '{branch_name}'"))?;
            let upstream_name = upstream.name()?.unwrap_or("unknown").to_string();
            (branch_name, upstream_name)
        };

        self.fetch_origin()?;

        let wt_repo = Repository::open(worktree_path)
            .with_context(|| format!("cannot re-open worktree at {}", worktree_path.display()))?;
        let upstream_oid = wt_repo
            .find_reference(&format!("refs/remotes/{upstream_name}"))
            .with_context(|| {
                format!("upstream ref 'refs/remotes/{upstream_name}' not found after fetch")
            })?
            .peel_to_commit()
            .context("upstream ref is not a commit")?
            .id();
        let annotated = wt_repo
            .find_annotated_commit(upstream_oid)
            .context("failed to find annotated commit for upstream")?;
        let (analysis, _) = wt_repo.merge_analysis(&[&annotated])?;

        if analysis.is_up_to_date() {
            return Ok(format!("'{branch_name}' is already up-to-date"));
        }
        if analysis.is_fast_forward() {
            let head_oid = wt_repo.head()?.peel_to_commit()?.id();
            let count = {
                let mut revwalk = wt_repo.revwalk()?;
                revwalk.push(upstream_oid)?;
                revwalk.hide(head_oid)?;
                revwalk.count()
            };
            // ref を先に動かすと checkout_head が「HEAD は既にそこ」と判断して
            // ファイルを更新しないので、tree を先に checkout してから ref を進める。
            let target = wt_repo.find_commit(upstream_oid)?;
            wt_repo.checkout_tree(
                target.as_object(),
                Some(git2::build::CheckoutBuilder::new().safe()),
            )?;
            wt_repo
                .find_reference(&format!("refs/heads/{branch_name}"))?
                .set_target(
                    upstream_oid,
                    &format!("conductor: fast-forward pull {upstream_name} into {branch_name}"),
                )?;
            return Ok(format!(
                "Pulled '{branch_name}': fast-forward ({count} commit(s))"
            ));
        }
        if analysis.is_normal() {
            return Ok(format!(
                "Cannot fast-forward '{branch_name}'. Manual merge needed"
            ));
        }
        anyhow::bail!("pull: unexpected merge analysis result for '{branch_name}'");
    }

    /// branch_name を main_branch へ fast-forward マージする。fast-forward できなければ
    /// 手動マージの案内を返す。
    pub fn merge_into_main(&self, branch_name: &str, main_branch: &str) -> Result<String> {
        let main_path = self.main_worktree_path()?;
        let main_repo = Repository::open(&main_path)
            .with_context(|| format!("cannot open main worktree at {}", main_path.display()))?;
        Self::save_orig_head(&main_repo, "conductor: save ORIG_HEAD before merge");

        let branch_oid = main_repo
            .find_branch(branch_name, git2::BranchType::Local)
            .with_context(|| format!("branch '{branch_name}' not found"))?
            .get()
            .peel_to_commit()?
            .id();
        let annotated = main_repo
            .find_annotated_commit(branch_oid)
            .context("failed to find annotated commit for branch")?;
        let (analysis, _) = main_repo.merge_analysis(&[&annotated])?;

        if analysis.is_up_to_date() {
            return Ok(format!(
                "{main_branch} is already up-to-date with {branch_name}."
            ));
        }
        if analysis.is_fast_forward() {
            main_repo
                .find_reference(&format!("refs/heads/{main_branch}"))?
                .set_target(
                    branch_oid,
                    &format!("conductor: fast-forward merge {branch_name} into {main_branch}"),
                )?;
            main_repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))?;
            return Ok(format!(
                "Fast-forward merged {branch_name} into {main_branch}."
            ));
        }
        if analysis.is_normal() {
            return Ok(format!(
                "Cannot fast-forward. Manual merge needed: cd {} && git merge {}",
                main_path.display(),
                branch_name
            ));
        }
        anyhow::bail!("merge analysis returned unexpected result for {branch_name}");
    }

    /// `git reset --hard origin/<main_branch>` を main worktree で行う。
    pub fn reset_main_to_origin(&self, main_branch: &str) -> Result<String> {
        let main_path = self.main_worktree_path()?;
        let main_repo = Repository::open(&main_path)
            .with_context(|| format!("cannot open main worktree at {}", main_path.display()))?;
        Self::save_orig_head(&main_repo, "conductor: save ORIG_HEAD before reset");

        let remote_ref_name = format!("refs/remotes/origin/{main_branch}");
        let remote_commit = main_repo
            .find_reference(&remote_ref_name)
            .with_context(|| {
                format!("remote ref '{remote_ref_name}' not found. Have you fetched?")
            })?
            .peel_to_commit()
            .context("remote ref does not point to a commit")?;
        main_repo
            .reset(remote_commit.as_object(), git2::ResetType::Hard, None)
            .context("failed to hard reset")?;

        Ok(format!(
            "Reset {main_branch} to origin/{main_branch} (commit {}).",
            &remote_commit.id().to_string()[..8]
        ))
    }

    /// ベストエフォート。HEAD が無い (unborn) なら何もしない。
    fn save_orig_head(repo: &Repository, log_message: &str) {
        if let Ok(head) = repo.head()
            && let Ok(commit) = head.peel_to_commit()
        {
            let _ = repo.reference("refs/original/ORIG_HEAD", commit.id(), true, log_message);
        }
    }
}
