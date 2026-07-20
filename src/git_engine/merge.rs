//! Pulling (fetch + fast-forward), merging a branch into main, and hard
//! resetting main to origin.

use anyhow::{Context, Result};
use git2::Repository;

use super::GitEngine;

impl GitEngine {
    // ── Pull (fetch + fast-forward) ────────────────────────────────────

    /// Fetch from origin and fast-forward the branch in the given worktree.
    ///
    /// Returns a human-readable status message describing the outcome.
    /// Only fast-forward merges are performed; non-FF situations are reported
    /// so the user can resolve them manually.
    ///
    /// NOTE: calls `fetch_origin()` internally, so this performs network I/O.
    /// Must be called from a background thread.
    pub fn pull_worktree(&self, worktree_path: &std::path::Path) -> Result<String> {
        let wt_repo = Repository::open(worktree_path)
            .with_context(|| format!("cannot open worktree at {}", worktree_path.display()))?;

        // Ensure HEAD points to a branch (not detached).
        let head = wt_repo.head().context("cannot read HEAD")?;
        if !head.is_branch() {
            anyhow::bail!("Cannot pull: HEAD is detached");
        }
        let branch_name = head.shorthand().unwrap_or("unknown").to_string();

        // Ensure the branch has an upstream configured.
        let local_branch = wt_repo
            .find_branch(&branch_name, git2::BranchType::Local)
            .with_context(|| format!("branch '{branch_name}' not found"))?;
        let upstream = local_branch
            .upstream()
            .with_context(|| format!("No upstream configured for '{branch_name}'"))?;
        let upstream_name = upstream.name()?.unwrap_or("unknown").to_string();

        // Fetch from origin (updates all remote refs).
        self.fetch_origin()?;

        // Re-open the repo to pick up the updated remote refs.
        let wt_repo = Repository::open(worktree_path)
            .with_context(|| format!("cannot re-open worktree at {}", worktree_path.display()))?;

        // Resolve upstream OID after fetch.
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

        // Merge analysis.
        let (analysis, _preference) = wt_repo.merge_analysis(&[&annotated])?;

        if analysis.is_up_to_date() {
            return Ok(format!("'{branch_name}' is already up-to-date"));
        }

        if analysis.is_fast_forward() {
            // Count commits for the status message.
            let head_oid = wt_repo.head()?.peel_to_commit()?.id();
            let count = {
                let mut revwalk = wt_repo.revwalk()?;
                revwalk.push(upstream_oid)?;
                revwalk.hide(head_oid)?;
                revwalk.count()
            };

            // Update working directory & index first, then move branch ref.
            // (checkout_tree works on the target tree directly, avoiding stale
            //  HEAD state that can cause checkout_head to skip file updates.)
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

    // ── Merge / Reset operations ─────────────────────────────────────

    /// Merge `branch_name` into the main branch using a fast-forward-only merge.
    ///
    /// Steps:
    /// 1. Record ORIG_HEAD for safety
    /// 2. Attempt fast-forward merge; if not possible, attempt a normal merge
    /// 3. If conflicts occur, abort and report
    ///
    /// Returns a description of what happened.
    pub fn merge_into_main(&self, branch_name: &str, main_branch: &str) -> Result<String> {
        let main_path = self.main_worktree_path()?;
        let main_repo = Repository::open(&main_path)
            .with_context(|| format!("cannot open main worktree at {}", main_path.display()))?;

        // Record ORIG_HEAD for safety
        let head = main_repo.head().context("no HEAD on main worktree")?;
        let head_commit = head.peel_to_commit().context("HEAD is not a commit")?;
        main_repo
            .reference(
                "refs/original/ORIG_HEAD",
                head_commit.id(),
                true,
                "conductor: save ORIG_HEAD before merge",
            )
            .ok(); // best-effort

        // Find the branch to merge
        let branch_ref = main_repo
            .find_branch(branch_name, git2::BranchType::Local)
            .with_context(|| format!("branch '{branch_name}' not found"))?;
        let branch_commit_oid = branch_ref.get().peel_to_commit()?.id();
        let branch_annotated = main_repo
            .find_annotated_commit(branch_commit_oid)
            .context("failed to find annotated commit for branch")?;

        // Perform merge analysis
        let (analysis, _preference) = main_repo.merge_analysis(&[&branch_annotated])?;

        if analysis.is_up_to_date() {
            return Ok(format!(
                "{main_branch} is already up-to-date with {branch_name}."
            ));
        }

        if analysis.is_fast_forward() {
            // Fast-forward: just move the main branch ref
            let mut main_ref = main_repo.find_reference(&format!("refs/heads/{main_branch}"))?;
            main_ref.set_target(
                branch_commit_oid,
                &format!("conductor: fast-forward merge {branch_name} into {main_branch}"),
            )?;
            // Update HEAD / working directory
            main_repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))?;
            return Ok(format!(
                "Fast-forward merged {branch_name} into {main_branch}."
            ));
        }

        if analysis.is_normal() {
            // Normal merge — this is more complex and can conflict.
            // For safety, we'll report that a non-fast-forward merge is needed
            // and recommend the user do it manually.
            return Ok(format!(
                "Cannot fast-forward. Manual merge needed: cd {} && git merge {}",
                main_path.display(),
                branch_name
            ));
        }

        anyhow::bail!("merge analysis returned unexpected result for {branch_name}");
    }

    /// Hard-reset the main branch to `origin/<main_branch>`.
    ///
    /// This is equivalent to: `cd <main_worktree> && git reset --hard origin/<main_branch>`
    pub fn reset_main_to_origin(&self, main_branch: &str) -> Result<String> {
        let main_path = self.main_worktree_path()?;
        let main_repo = Repository::open(&main_path)
            .with_context(|| format!("cannot open main worktree at {}", main_path.display()))?;

        // Record ORIG_HEAD for safety
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

        // Find origin/<main_branch>
        let remote_ref_name = format!("refs/remotes/origin/{main_branch}");
        let remote_ref = main_repo
            .find_reference(&remote_ref_name)
            .with_context(|| {
                format!("remote ref '{remote_ref_name}' not found. Have you fetched?")
            })?;
        let remote_commit = remote_ref
            .peel_to_commit()
            .context("remote ref does not point to a commit")?;

        // Reset to the remote commit
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
