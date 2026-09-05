//! HEAD やブランチから遡るコミットの一覧。

use anyhow::{Context, Result};
use chrono::Utc;
use git2::{Oid, Repository};

use super::{CommitInfo, GitEngine};
use crate::diff_state::short_oid;

impl GitEngine {
    /// 開いた worktree の HEAD から新しい順に、skip 件飛ばして最大 limit 件。
    ///
    /// ページは毎回 tip から歩き直す。途中のコミットから歩き直すと、トポロジカル順で
    /// 後に来るはずの別の枝が抜ける。
    pub fn head_log(&self, skip: usize, limit: usize) -> Result<Vec<CommitInfo>> {
        let tip = self
            .repo
            .head()
            .context("cannot resolve HEAD")?
            .peel_to_commit()
            .context("cannot peel HEAD to commit")?;
        walk(&self.repo, tip.id(), skip, limit)
    }

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
        walk(&self.repo, tip.id(), 0, limit)
    }
}

fn walk(repo: &Repository, tip: Oid, skip: usize, limit: usize) -> Result<Vec<CommitInfo>> {
    let mut revwalk = repo.revwalk()?;
    revwalk.push(tip)?;
    // 同じ秒に作られたコミットは TIME だけでは順序が定まらない。
    revwalk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)?;

    let now = Utc::now();
    let mut commits = Vec::new();
    for oid in revwalk.skip(skip).take(limit) {
        let commit = repo.find_commit(oid?)?;
        let full_oid = commit.id().to_string();
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

pub(super) fn first_line(message: Option<&str>) -> &str {
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
