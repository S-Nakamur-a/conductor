//! Branch deletion, worktree removal, and pruning stale worktree entries.

use std::path::Path;

use anyhow::{Context, Result};

use super::GitEngine;

impl GitEngine {
    // ── Enhanced deletion (wt rm -b -f) ─────────────────────────

    /// Delete a local branch by name. If `force` is true, uses -D (deletes
    /// even if not fully merged).
    pub fn delete_branch(&self, name: &str, force: bool) -> Result<()> {
        let mut branch = self
            .repo
            .find_branch(name, git2::BranchType::Local)
            .with_context(|| format!("branch '{name}' not found"))?;
        if force {
            // Force-delete: just delete the reference directly.
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

    /// Return `true` when every commit on `branch` is already contained in
    /// `into` (tip equal or an ancestor of `into`'s tip). Used to warn before
    /// a worktree delete force-removes a branch whose commits would become
    /// unreachable — libgit2's non-force `Branch::delete` does *not* perform
    /// the "not fully merged" refusal that the git CLI does, so this check is
    /// the only guard.
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

    /// Forcefully remove a worktree even if dirty.
    #[allow(dead_code)]
    pub fn remove_worktree_force(&self, worktree_path: &Path) -> Result<()> {
        let name = self
            .find_worktree_name_by_path(worktree_path)
            .with_context(|| format!("no worktree found for path {}", worktree_path.display()))?;
        let wt = self
            .repo
            .find_worktree(&name)
            .with_context(|| format!("worktree '{name}' not found"))?;

        let wt_path = wt.path().to_path_buf();

        // Prune with all flags to force removal.
        wt.prune(Some(
            git2::WorktreePruneOptions::new()
                .working_tree(true)
                .valid(true)
                .locked(true),
        ))
        .with_context(|| format!("failed to force-prune worktree '{name}'"))?;

        // Remove directory.
        if wt_path.exists() {
            std::fs::remove_dir_all(&wt_path)
                .with_context(|| format!("failed to remove directory {}", wt_path.display()))?;
        }

        Ok(())
    }

    // ── Prune stale worktrees (wt prune) ─────────────────────────

    /// Find worktree entries whose directories no longer exist (stale).
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

    /// Prune a single stale worktree entry.
    pub fn prune_stale_worktree(&self, name: &str) -> Result<()> {
        let wt = self
            .repo
            .find_worktree(name)
            .with_context(|| format!("worktree '{name}' not found"))?;

        wt.prune(Some(git2::WorktreePruneOptions::new().working_tree(true)))
            .with_context(|| format!("failed to prune stale worktree '{name}'"))?;

        Ok(())
    }

    /// Remove a linked worktree by name.
    ///
    /// This prunes the worktree entry and optionally removes the directory.
    /// Cannot remove the main worktree.
    pub fn remove_worktree(&self, worktree_path: &Path) -> Result<()> {
        let name = self
            .find_worktree_name_by_path(worktree_path)
            .with_context(|| format!("no worktree found for path {}", worktree_path.display()))?;
        let wt = self
            .repo
            .find_worktree(&name)
            .with_context(|| format!("worktree '{name}' not found"))?;

        let wt_path = wt.path().to_path_buf();

        // Validate it first
        if wt.validate().is_ok() {
            // Worktree is valid and exists — prune it
            wt.prune(Some(
                git2::WorktreePruneOptions::new()
                    .working_tree(true)
                    .valid(true),
            ))
            .with_context(|| format!("failed to prune worktree '{name}'"))?;
        } else {
            // Worktree is already invalid (e.g. directory deleted) — just prune
            wt.prune(Some(git2::WorktreePruneOptions::new().working_tree(true)))
                .with_context(|| format!("failed to prune worktree '{name}'"))?;
        }

        // Remove the directory if it still exists
        if wt_path.exists() {
            std::fs::remove_dir_all(&wt_path).with_context(|| {
                format!("failed to remove worktree directory {}", wt_path.display())
            })?;
        }

        Ok(())
    }

    /// Find the libgit2 worktree name that corresponds to the given path.
    ///
    /// Worktree names may differ from branch names (e.g. `feature/foo`
    /// creates a worktree named `foo`), so we iterate all registered
    /// worktrees and match by path.
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
