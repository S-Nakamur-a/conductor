//! The `wt grab`/`wt ungrab` branch-swap workflow: moving a branch checked
//! out in one worktree onto another, and reversing it, with persisted state
//! for crash recovery and zsh `wt` compatibility.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use git2::Repository;

use super::GitEngine;

impl GitEngine {
    // ── Grab / Ungrab ──────────────────────────────────────────────

    /// Check whether a worktree has uncommitted changes to tracked files.
    ///
    /// Uses `git diff --quiet HEAD` (shell-out) to match the behaviour of the
    /// `wt grab` zsh helper exactly.  libgit2's status API can report extra
    /// entries (renames, type-changes, ignored-file edge-cases) that
    /// `git diff HEAD` does not, causing false positives.
    pub fn has_tracked_changes(&self, worktree_path: &Path) -> Result<bool> {
        use std::process::{Command, Stdio};

        let status = Command::new("git")
            .args(["diff", "--quiet", "HEAD"])
            .current_dir(worktree_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("failed to run `git diff --quiet HEAD`")?;

        // exit 0 = clean, exit 1 = dirty
        Ok(!status.success())
    }

    /// Return the git common dir (`.git/` for main, resolved via commondir
    /// for linked worktrees).  This is the shared directory where refs,
    /// objects, and our `wt-grab` state file live.
    pub fn git_common_dir(&self) -> Result<PathBuf> {
        let git_dir = self.repo.path(); // .git/ or .git/worktrees/<name>/
        // Check for .git/worktrees/<name>/commondir which points to the shared .git/.
        let commondir_file = git_dir.join("commondir");
        if commondir_file.exists() {
            let content =
                std::fs::read_to_string(&commondir_file).context("failed to read commondir")?;
            let relative = content.trim();
            let resolved = git_dir.join(relative);
            return Ok(resolved.canonicalize().unwrap_or(resolved));
        }
        // Already the main repo's .git/ directory.
        Ok(git_dir.to_path_buf())
    }

    /// Persist grab state to `$git_common_dir/wt-grab`.
    /// Format: 3 mandatory lines (branch, worktree path, stash branch)
    /// plus an optional 4th line (Claude Code session ID for resume).
    pub fn save_grab_state(
        &self,
        branch: &str,
        source_worktree_path: &Path,
        stash_branch: &str,
        claude_session_id: Option<&str>,
    ) -> Result<()> {
        let grab_file = self.git_common_dir()?.join("wt-grab");
        let mut content = format!(
            "{}\n{}\n{}\n",
            branch,
            source_worktree_path.display(),
            stash_branch,
        );
        if let Some(session_id) = claude_session_id {
            content.push_str(session_id);
            content.push('\n');
        }
        std::fs::write(&grab_file, content)
            .with_context(|| format!("failed to write {}", grab_file.display()))
    }

    /// Load grab state from `$git_common_dir/wt-grab`.
    /// Returns `(branch, source_worktree_path, stash_branch, claude_session_id)` or `None`
    /// if the file does not exist. The 4th field (session ID) is optional.
    #[allow(clippy::type_complexity)]
    pub fn load_grab_state(&self) -> Result<Option<(String, PathBuf, String, Option<String>)>> {
        let grab_file = self.git_common_dir()?.join("wt-grab");
        if !grab_file.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&grab_file)
            .with_context(|| format!("failed to read {}", grab_file.display()))?;
        let mut lines = content.lines();
        let branch = lines
            .next()
            .ok_or_else(|| anyhow!("wt-grab: missing branch line"))?
            .to_string();
        let wt_path = PathBuf::from(
            lines
                .next()
                .ok_or_else(|| anyhow!("wt-grab: missing worktree path line"))?,
        );
        let stash_branch = lines
            .next()
            .ok_or_else(|| anyhow!("wt-grab: missing stash branch line"))?
            .to_string();
        let claude_session_id = lines
            .next()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        Ok(Some((branch, wt_path, stash_branch, claude_session_id)))
    }

    /// Remove the `$git_common_dir/wt-grab` state file.
    pub fn remove_grab_state(&self) -> Result<()> {
        let grab_file = self.git_common_dir()?.join("wt-grab");
        if grab_file.exists() {
            std::fs::remove_file(&grab_file)
                .with_context(|| format!("failed to remove {}", grab_file.display()))?;
        }
        Ok(())
    }

    /// Grab a branch: move the source worktree to a temporary `__grab`
    /// branch, then checkout main to the original branch.
    ///
    /// Requires both worktrees to have no uncommitted tracked changes.
    /// Persists state to `$git_common_dir/wt-grab`.
    pub fn grab_branch(
        &self,
        main_path: &Path,
        source_worktree_path: &Path,
        branch_name: &str,
        claude_session_id: Option<&str>,
    ) -> Result<()> {
        if self.has_tracked_changes(main_path)? {
            anyhow::bail!("Main worktree has uncommitted tracked changes. Commit or stash first.");
        }
        if self.has_tracked_changes(source_worktree_path)? {
            anyhow::bail!(
                "Worktree '{branch_name}' has uncommitted tracked changes. Commit or stash first."
            );
        }

        let grab_branch_name = format!("{branch_name}__grab");

        // Create __grab branch on source worktree and checkout it.
        let source_repo = Repository::open(source_worktree_path).with_context(|| {
            format!("cannot open worktree at {}", source_worktree_path.display())
        })?;
        let head_commit = source_repo.head()?.peel_to_commit()?;
        source_repo
            .branch(&grab_branch_name, &head_commit, false)
            .with_context(|| format!("failed to create branch '{grab_branch_name}'"))?;
        source_repo
            .set_head(&format!("refs/heads/{grab_branch_name}"))
            .with_context(|| format!("failed to set HEAD to '{grab_branch_name}'"))?;
        source_repo
            .checkout_head(Some(git2::build::CheckoutBuilder::default().force()))
            .context("failed to checkout __grab branch")?;

        // Checkout main worktree to the original branch.
        let main_repo = Repository::open(main_path)
            .with_context(|| format!("cannot open main worktree at {}", main_path.display()))?;
        main_repo
            .set_head(&format!("refs/heads/{branch_name}"))
            .with_context(|| format!("failed to set main HEAD to '{branch_name}'"))?;
        main_repo
            .checkout_head(Some(git2::build::CheckoutBuilder::default().force()))
            .with_context(|| format!("failed to checkout '{branch_name}' in main worktree"))?;

        // Persist grab state for crash recovery and zsh `wt` compatibility.
        self.save_grab_state(
            branch_name,
            source_worktree_path,
            &grab_branch_name,
            claude_session_id,
        )?;

        Ok(())
    }

    /// Ungrab: return main to main branch, restore source worktree to
    /// original branch, and delete the temporary `__grab` branch.
    ///
    /// Requires both worktrees to have no uncommitted tracked changes.
    /// Uses `set_head` + hard `reset` so that the index and working tree
    /// are reliably updated even when commits have been added after grab.
    pub fn ungrab_branch(
        &self,
        main_path: &Path,
        source_worktree_path: &Path,
        branch_name: &str,
        main_branch: &str,
    ) -> Result<()> {
        if self.has_tracked_changes(main_path)? {
            anyhow::bail!("Main worktree has uncommitted tracked changes. Commit or stash first.");
        }
        if self.has_tracked_changes(source_worktree_path)? {
            anyhow::bail!(
                "Worktree (on __grab) has uncommitted tracked changes. Commit or stash first."
            );
        }

        // Checkout main worktree back to main branch.
        // Scope the main_repo so it is dropped (and file handles released)
        // before we open the source worktree repo.
        {
            let main_repo = Repository::open(main_path)
                .with_context(|| format!("cannot open main worktree at {}", main_path.display()))?;
            main_repo
                .set_head(&format!("refs/heads/{main_branch}"))
                .with_context(|| format!("failed to set main HEAD to '{main_branch}'"))?;
            let head_commit = main_repo.head()?.peel_to_commit()?;
            main_repo
                .reset(head_commit.as_object(), git2::ResetType::Hard, None)
                .with_context(|| format!("failed to reset main worktree to '{main_branch}'"))?;
        }

        // Checkout source worktree back to original branch.
        let source_repo = Repository::open(source_worktree_path).with_context(|| {
            format!("cannot open worktree at {}", source_worktree_path.display())
        })?;
        source_repo
            .set_head(&format!("refs/heads/{branch_name}"))
            .with_context(|| format!("failed to set HEAD to '{branch_name}'"))?;
        let head_commit = source_repo.head()?.peel_to_commit()?;
        source_repo
            .reset(head_commit.as_object(), git2::ResetType::Hard, None)
            .with_context(|| format!("failed to reset worktree to '{branch_name}'"))?;

        // Delete the temporary __grab branch.
        let grab_branch_name = format!("{branch_name}__grab");
        if let Ok(mut grab_branch) =
            source_repo.find_branch(&grab_branch_name, git2::BranchType::Local)
        {
            let _ = grab_branch.delete();
        }

        // Remove the persisted grab state file.
        self.remove_grab_state()?;

        Ok(())
    }
}
