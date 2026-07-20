//! Listing recent commits on a branch and cherry-picking one of them into a
//! worktree.

use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;
use git2::Repository;

use super::{CommitInfo, GitEngine};

impl GitEngine {
    // ── Cherry-pick helpers ───────────────────────────────────────────

    /// List up to `limit` commits from the given branch, newest first.
    pub fn list_branch_commits(&self, branch_name: &str, limit: usize) -> Result<Vec<CommitInfo>> {
        let branch = self
            .repo
            .find_branch(branch_name, git2::BranchType::Local)
            .with_context(|| format!("branch '{branch_name}' not found"))?;
        let commit = branch
            .get()
            .peel_to_commit()
            .with_context(|| format!("cannot resolve branch '{branch_name}' to a commit"))?;

        let mut revwalk = self.repo.revwalk()?;
        revwalk.push(commit.id())?;
        revwalk.set_sorting(git2::Sort::TIME)?;

        let now = Utc::now();
        let mut commits = Vec::new();

        for oid_result in revwalk {
            if commits.len() >= limit {
                break;
            }
            let oid = oid_result?;
            let c = self.repo.find_commit(oid)?;

            let full_oid = oid.to_string();
            let short_oid = full_oid[..8.min(full_oid.len())].to_string();

            let message = c
                .message()
                .unwrap_or("")
                .lines()
                .next()
                .unwrap_or("")
                .to_string();

            let author = c.author().name().unwrap_or("unknown").to_string();

            let secs = c.time().seconds();
            let commit_time = chrono::TimeZone::timestamp_opt(&Utc, secs, 0)
                .single()
                .unwrap_or_else(Utc::now);
            let duration = now.signed_duration_since(commit_time);
            let time_ago = Self::format_duration_ago(duration);

            commits.push(CommitInfo {
                short_oid,
                oid: full_oid,
                message,
                author,
                time_ago,
            });
        }

        Ok(commits)
    }

    /// Cherry-pick a commit (identified by OID hex string) into the repo
    /// at `worktree_path`.
    ///
    /// On success, creates a new commit with the original message and returns
    /// a success description. If conflicts arise, aborts and returns an error
    /// message.
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

        // Perform the cherry-pick (applies changes to index and workdir).
        repo.cherrypick(&commit, None)
            .with_context(|| format!("cherry-pick failed for {commit_oid_str}"))?;

        // Check for conflicts.
        let index = repo.index()?;
        if index.has_conflicts() {
            // Abort by cleaning up the cherry-pick state.
            repo.cleanup_state()?;
            // Reset workdir to HEAD to undo partial changes.
            let head = repo.head()?.peel_to_commit()?;
            repo.reset(head.as_object(), git2::ResetType::Hard, None)?;
            return Ok(format!(
                "Cherry-pick of {} aborted due to conflicts.",
                &commit_oid_str[..8.min(commit_oid_str.len())]
            ));
        }

        // No conflicts — create a commit.
        let mut index = repo.index()?;
        let tree_oid = index.write_tree()?;
        let tree = repo.find_tree(tree_oid)?;
        let head_commit = repo.head()?.peel_to_commit()?;

        let original_message = commit.message().unwrap_or("cherry-picked commit");
        let sig = repo
            .signature()
            .or_else(|_| git2::Signature::now("Conductor", "conductor@localhost"))
            .context("cannot create signature")?;

        repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            original_message,
            &tree,
            &[&head_commit],
        )?;

        // Clean up cherry-pick state.
        repo.cleanup_state()?;

        let short = &commit_oid_str[..8.min(commit_oid_str.len())];
        let msg_first_line = commit.message().unwrap_or("").lines().next().unwrap_or("");
        Ok(format!(
            "Cherry-picked {short} \"{msg_first_line}\" successfully."
        ))
    }

    // ── Internal helpers ─────────────────────────────────────────────

    /// Resolve a ref string (branch name, remote ref, or tag) to a `Commit`.
    #[allow(dead_code)]
    fn resolve_ref_to_commit(&self, refspec: &str) -> Result<git2::Commit<'_>> {
        // Try as a direct reference first (e.g. "refs/remotes/origin/main").
        if let Ok(reference) = self.repo.find_reference(&format!("refs/remotes/{refspec}")) {
            return reference
                .peel_to_commit()
                .with_context(|| format!("ref '{refspec}' does not point to a commit"));
        }
        if let Ok(reference) = self.repo.find_reference(&format!("refs/heads/{refspec}")) {
            return reference
                .peel_to_commit()
                .with_context(|| format!("ref '{refspec}' does not point to a commit"));
        }
        // Try revparse as a fallback.
        let obj = self
            .repo
            .revparse_single(refspec)
            .with_context(|| format!("cannot resolve '{refspec}'"))?;
        obj.peel_to_commit()
            .with_context(|| format!("'{refspec}' does not point to a commit"))
    }

    /// Format a `chrono::Duration` as a human-readable "X ago" string.
    fn format_duration_ago(duration: chrono::Duration) -> String {
        let seconds = duration.num_seconds();
        if seconds < 0 {
            return "just now".to_string();
        }
        let minutes = duration.num_minutes();
        let hours = duration.num_hours();
        let days = duration.num_days();
        let weeks = days / 7;
        let months = days / 30;

        if seconds < 60 {
            format!("{seconds}s ago")
        } else if minutes < 60 {
            format!("{minutes}m ago")
        } else if hours < 24 {
            format!("{hours}h ago")
        } else if days < 7 {
            format!("{days}d ago")
        } else if weeks < 5 {
            format!("{weeks}w ago")
        } else {
            format!("{months}mo ago")
        }
    }
}
