//! Worktree creation (from a base ref, an existing branch, or a remote
//! branch) and keeping a base ref reasonably up to date.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};

use super::GitEngine;

impl GitEngine {
    // ── Worktree creation / deletion ─────────────────────────────

    /// Create a new worktree branching from a base ref (wt new equivalent).
    ///
    /// `branch_name` is the new local branch name.
    /// `base_ref` is the starting point (e.g. "origin/main").
    /// `worktree_dir_override` is an optional custom base directory for worktrees
    /// (from config `general.worktree_dir`).
    /// The worktree is placed at `<base_dir>/<dir_name>`.
    pub fn create_worktree_from_base(
        &self,
        branch_name: &str,
        base_ref: &str,
        worktree_dir_override: Option<&Path>,
    ) -> Result<PathBuf> {
        // Prevent accidental origin/ prefix on branch name.
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

        // Force-prune any existing worktree entry with this name.
        self.force_prune_worktree_entry(dir_name);

        // Use `git worktree add` CLI — more reliable than libgit2's worktree API.
        // We use spawn()+wait() instead of output() to avoid blocking on
        // post-checkout hooks that spawn background processes (e.g. `npm ci &`).
        // output() reads pipes until EOF, which blocks if background processes
        // inherit the pipe FDs. wait() only waits for the child process to exit.
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
            // Safe to read stderr: if git failed, post-checkout hook didn't run,
            // so no background processes hold the pipe open.
            let mut stderr_buf = String::new();
            if let Some(mut stderr) = child.stderr.take() {
                use std::io::Read;
                let _ = stderr.read_to_string(&mut stderr_buf);
            }
            anyhow::bail!("git worktree add failed: {}", stderr_buf.trim());
        }

        Ok(wt_path)
    }

    /// Create a worktree checking out a branch that already exists locally
    /// (PR intake equivalent — the branch was created by a prior
    /// `fetch_refspec`, so this is a plain `git worktree add <path> <branch>`
    /// with no `-b`, unlike `create_worktree_from_base`/`create_worktree_from_remote`).
    ///
    /// `wt_dir` is the full worktree directory to create (the caller decides
    /// its name/location, e.g. via `worktrees_base_dir`).
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

        // See create_worktree_from_base() for why we use spawn()+wait() over output().
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

    /// Ensure a local branch for `base_branch` exists and is reasonably
    /// up to date, without ever force-updating or checking it out.
    ///
    /// - If no local branch exists yet, fetch it straight into
    ///   `refs/heads/<base_branch>`.
    /// - If it exists, fetch the remote tip into a scratch ref first (fetching
    ///   directly into `refs/heads/<base_branch>` would be rejected by git if
    ///   that branch happens to be checked out in another worktree) and only
    ///   fast-forward the local branch if the fetched tip is a strict
    ///   descendant of it. A non-fast-forward (diverged, or already caught
    ///   up) is left untouched — this is metadata for a diff base, not a
    ///   branch the user is actively working on, so silently discarding local
    ///   history would be the wrong failure mode.
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

    // ── Remote branch operations (wt switch) ─────────────────────

    /// List remote branches (refs/remotes/origin/*), excluding HEAD.
    pub fn list_remote_branches(&self) -> Result<Vec<String>> {
        let mut names = Vec::new();
        let branches = self.repo.branches(Some(git2::BranchType::Remote))?;
        for branch in branches {
            let (branch, _) = branch?;
            if let Some(name) = branch.name()? {
                // Skip origin/HEAD.
                if name.ends_with("/HEAD") {
                    continue;
                }
                names.push(name.to_string());
            }
        }
        names.sort();
        Ok(names)
    }

    /// Resolve the best existing starting ref for a new worktree branching off
    /// `main_branch`.
    ///
    /// Prefers the remote-tracking branch `origin/<main_branch>`, falls back to
    /// the local branch `<main_branch>`, then to `HEAD`. The returned ref is
    /// guaranteed to resolve, so `git worktree add ... <ref>` won't fail with an
    /// "invalid reference" error in a repo that has no remote.
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

    /// Create a worktree from a remote branch (wt switch equivalent).
    ///
    /// `remote_branch` should be like "origin/feature-x".
    /// `worktree_dir_override` is an optional custom base directory for worktrees
    /// (from config `general.worktree_dir`).
    /// Creates a local tracking branch and sets upstream.
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

        // Force-prune any existing worktree entry with this name.
        self.force_prune_worktree_entry(dir_name);

        // Use `git worktree add` CLI — more reliable than libgit2's worktree API
        // which can fail in various edge cases (stale locks, index issues, etc.).
        // See create_worktree_from_base() for why we use spawn()+wait() over output().
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

    /// Force-prune a worktree entry by name, regardless of validity.
    /// Best-effort: silently ignores errors (entry may not exist).
    /// Used before creating a new worktree to clean up lingering entries.
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
