//! ブランチの直近コミットの一覧と、その 1 つを別の worktree へ cherry-pick する処理。

use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;
use git2::Repository;

use super::{CommitInfo, GitEngine};

impl GitEngine {
    /// branch_name のコミットを新しい順に最大 limit 件返す。
    pub fn list_branch_commits(&self, branch_name: &str, limit: usize) -> Result<Vec<CommitInfo>> {
        let branch = self
            .repo
            .find_branch(branch_name, git2::BranchType::Local)
            .with_context(|| format!("branch '{branch_name}' not found"))?;
        let tip = branch
            .get()
            .peel_to_commit()
            .with_context(|| format!("cannot resolve branch '{branch_name}' to a commit"))?;

        let mut revwalk = self.repo.revwalk()?;
        revwalk.push(tip.id())?;
        // 同じ秒に作られたコミットは TIME だけでは順序が定まらない。
        revwalk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)?;

        let now = Utc::now();
        let mut commits = Vec::new();
        for oid in revwalk.take(limit) {
            let oid = oid?;
            let commit = self.repo.find_commit(oid)?;
            let full_oid = oid.to_string();
            let commit_time = chrono::TimeZone::timestamp_opt(&Utc, commit.time().seconds(), 0)
                .single()
                .unwrap_or(now);
            commits.push(CommitInfo {
                short_oid: short_oid(&full_oid).to_string(),
                oid: full_oid,
                message: first_line(commit.message()).to_string(),
                author: commit.author().name().unwrap_or("unknown").to_string(),
                time_ago: format_duration_ago(now.signed_duration_since(commit_time)),
            });
        }
        Ok(commits)
    }

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

fn short_oid(oid: &str) -> &str {
    &oid[..8.min(oid.len())]
}

fn first_line(message: Option<&str>) -> &str {
    message.unwrap_or("").lines().next().unwrap_or("")
}

pub(super) fn format_duration_ago(duration: chrono::Duration) -> String {
    let seconds = duration.num_seconds();
    if seconds < 0 {
        return "just now".to_string();
    }
    let minutes = duration.num_minutes();
    let hours = duration.num_hours();
    let days = duration.num_days();
    if seconds < 60 {
        format!("{seconds}s ago")
    } else if minutes < 60 {
        format!("{minutes}m ago")
    } else if hours < 24 {
        format!("{hours}h ago")
    } else if days < 7 {
        format!("{days}d ago")
    } else if days < 35 {
        format!("{}w ago", days / 7)
    } else {
        format!("{}mo ago", days / 30)
    }
}
