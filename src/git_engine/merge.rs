//! pull(fetch + fast-forward)、ブランチを main へマージする処理、
//! main を origin へハードリセットする処理。

use anyhow::{Context, Result};
use git2::Repository;

use super::GitEngine;

impl GitEngine {
    // Pull (fetch + fast-forward)

    /// origin から fetch し、指定した worktree のブランチを fast-forward する。
    ///
    /// 結果を説明する人間が読めるステータスメッセージを返す。fast-forward
    /// マージのみを行い、non-FF な状況はユーザが手動で解決できるよう報告する。
    ///
    /// NOTE: 内部で fetch_origin() を呼ぶのでネットワーク I/O が発生する。
    /// バックグラウンドスレッドから呼ぶこと。
    pub fn pull_worktree(&self, worktree_path: &std::path::Path) -> Result<String> {
        let wt_repo = Repository::open(worktree_path)
            .with_context(|| format!("cannot open worktree at {}", worktree_path.display()))?;

        // HEAD がブランチを指していることを確認する(detached でないこと)。
        let head = wt_repo.head().context("cannot read HEAD")?;
        if !head.is_branch() {
            anyhow::bail!("Cannot pull: HEAD is detached");
        }
        let branch_name = head.shorthand().unwrap_or("unknown").to_string();

        // ブランチに upstream が設定されていることを確認する。
        let local_branch = wt_repo
            .find_branch(&branch_name, git2::BranchType::Local)
            .with_context(|| format!("branch '{branch_name}' not found"))?;
        let upstream = local_branch
            .upstream()
            .with_context(|| format!("No upstream configured for '{branch_name}'"))?;
        let upstream_name = upstream.name()?.unwrap_or("unknown").to_string();

        // origin から fetch する(全リモート ref を更新する)。
        self.fetch_origin()?;

        // 更新されたリモート ref を取り込むためリポジトリを開き直す。
        let wt_repo = Repository::open(worktree_path)
            .with_context(|| format!("cannot re-open worktree at {}", worktree_path.display()))?;

        // fetch 後に upstream の OID を解決する。
        let upstream_ref = wt_repo
            .find_reference(&format!("refs/remotes/{upstream_name}"))
            .with_context(|| {
                format!("upstream ref 'refs/remotes/{upstream_name}' not found after fetch")
            })?;
        let upstream_oid = upstream_ref
            .peel_to_commit()
            .context("upstream ref is not a commit")?
            .id();
        let annotated = wt_repo
            .find_annotated_commit(upstream_oid)
            .context("failed to find annotated commit for upstream")?;

        // マージの分析を行う。
        let (analysis, _preference) = wt_repo.merge_analysis(&[&annotated])?;

        if analysis.is_up_to_date() {
            return Ok(format!("'{branch_name}' is already up-to-date"));
        }

        if analysis.is_fast_forward() {
            // ステータスメッセージ用にコミット数を数える。
            let head_oid = wt_repo.head()?.peel_to_commit()?.id();
            let count = {
                let mut revwalk = wt_repo.revwalk()?;
                revwalk.push(upstream_oid)?;
                revwalk.hide(head_oid)?;
                revwalk.count()
            };

            // 先に working directory と index を更新してから branch ref を動かす。
            // (checkout_tree は対象の tree に直接作用するので、checkout_head が
            //  ファイル更新をスキップする原因になる古い HEAD 状態を避けられる。)
            let target_commit = wt_repo.find_commit(upstream_oid)?;
            wt_repo.checkout_tree(
                target_commit.as_object(),
                Some(git2::build::CheckoutBuilder::new().safe()),
            )?;
            let mut branch_ref = wt_repo.find_reference(&format!("refs/heads/{branch_name}"))?;
            branch_ref.set_target(
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

    // Merge / Reset 操作

    /// fast-forward のみのマージで branch_name を main ブランチへマージする。
    ///
    /// 手順:
    /// 1. 安全のため ORIG_HEAD を記録する
    /// 2. fast-forward マージを試み、できなければ normal マージを試みる
    /// 3. コンフリクトが起きたら中止して報告する
    ///
    /// 結果の説明文を返す。
    pub fn merge_into_main(&self, branch_name: &str, main_branch: &str) -> Result<String> {
        let main_path = self.main_worktree_path()?;
        let main_repo = Repository::open(&main_path)
            .with_context(|| format!("cannot open main worktree at {}", main_path.display()))?;

        // 安全のため ORIG_HEAD を記録する
        let head = main_repo.head().context("no HEAD on main worktree")?;
        let head_commit = head.peel_to_commit().context("HEAD is not a commit")?;
        main_repo
            .reference(
                "refs/original/ORIG_HEAD",
                head_commit.id(),
                true,
                "conductor: save ORIG_HEAD before merge",
            )
            .ok(); // ベストエフォート

        // マージ対象のブランチを見つける
        let branch_ref = main_repo
            .find_branch(branch_name, git2::BranchType::Local)
            .with_context(|| format!("branch '{branch_name}' not found"))?;
        let branch_commit_oid = branch_ref.get().peel_to_commit()?.id();
        let branch_annotated = main_repo
            .find_annotated_commit(branch_commit_oid)
            .context("failed to find annotated commit for branch")?;

        // マージ分析を行う
        let (analysis, _preference) = main_repo.merge_analysis(&[&branch_annotated])?;

        if analysis.is_up_to_date() {
            return Ok(format!(
                "{main_branch} is already up-to-date with {branch_name}."
            ));
        }

        if analysis.is_fast_forward() {
            // fast-forward: main ブランチの ref を動かすだけ
            let mut main_ref = main_repo.find_reference(&format!("refs/heads/{main_branch}"))?;
            main_ref.set_target(
                branch_commit_oid,
                &format!("conductor: fast-forward merge {branch_name} into {main_branch}"),
            )?;
            // HEAD / working directory を更新する
            main_repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))?;
            return Ok(format!(
                "Fast-forward merged {branch_name} into {main_branch}."
            ));
        }

        if analysis.is_normal() {
            // normal マージ — これはより複雑でコンフリクトの可能性がある。
            // 安全のため non-fast-forward マージが必要であることを報告し、
            // ユーザに手動で行うよう案内する。
            return Ok(format!(
                "Cannot fast-forward. Manual merge needed: cd {} && git merge {}",
                main_path.display(),
                branch_name
            ));
        }

        anyhow::bail!("merge analysis returned unexpected result for {branch_name}");
    }

    /// main ブランチを origin/<main_branch> へハードリセットする。
    ///
    /// cd <main_worktree> && git reset --hard origin/<main_branch> と等価。
    pub fn reset_main_to_origin(&self, main_branch: &str) -> Result<String> {
        let main_path = self.main_worktree_path()?;
        let main_repo = Repository::open(&main_path)
            .with_context(|| format!("cannot open main worktree at {}", main_path.display()))?;

        // 安全のため ORIG_HEAD を記録する
        if let Ok(head) = main_repo.head()
            && let Ok(commit) = head.peel_to_commit()
        {
            main_repo
                .reference(
                    "refs/original/ORIG_HEAD",
                    commit.id(),
                    true,
                    "conductor: save ORIG_HEAD before reset",
                )
                .ok();
        }

        // origin/<main_branch> を見つける
        let remote_ref_name = format!("refs/remotes/origin/{main_branch}");
        let remote_ref = main_repo
            .find_reference(&remote_ref_name)
            .with_context(|| {
                format!("remote ref '{remote_ref_name}' not found. Have you fetched?")
            })?;
        let remote_commit = remote_ref
            .peel_to_commit()
            .context("remote ref does not point to a commit")?;

        // リモートのコミットへリセットする
        let obj = remote_commit.as_object();
        main_repo
            .reset(obj, git2::ResetType::Hard, None)
            .context("failed to hard reset")?;

        Ok(format!(
            "Reset {main_branch} to origin/{main_branch} (commit {}).",
            &remote_commit.id().to_string()[..8]
        ))
    }
}
