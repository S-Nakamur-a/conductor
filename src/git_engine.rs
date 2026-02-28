//! Git operations powered by libgit2.
//!
//! Provides a high-level interface over `git2` for repository inspection:
//! worktree listing, status counts, commit info, diff generation, and more.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use git2::{Repository, StatusOptions, StatusShow};

/// Info about a single worktree.
#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    /// Absolute path to the worktree root directory.
    pub path: PathBuf,
    /// Branch name checked out in this worktree (e.g. "main", "feature-x").
    pub branch: String,
    /// Whether this is the main (bare/primary) worktree.
    pub is_main: bool,
    /// Number of newly added (untracked or index-new) files.
    pub added: usize,
    /// Number of modified files (index or working directory).
    pub modified: usize,
    /// Number of deleted files (index or working directory).
    pub deleted: usize,
    /// True when the working directory has no uncommitted changes.
    pub is_clean: bool,
    /// Commits ahead of upstream (local commits not yet pushed). `None` if no upstream.
    pub ahead: Option<usize>,
    /// Commits behind upstream (remote commits not yet pulled). `None` if no upstream.
    pub behind: Option<usize>,
}

/// Summary info for a single commit.
#[derive(Debug, Clone)]
pub struct CommitInfo {
    /// Short hex OID (first 8 chars).
    pub short_oid: String,
    /// Full hex OID.
    pub oid: String,
    /// First line of commit message.
    pub message: String,
    /// Commit author name.
    pub author: String,
    /// Timestamp as a human-readable string.
    pub time_ago: String,
}

/// Wrapper around a `git2::Repository` that exposes conductor-specific helpers.
pub struct GitEngine {
    repo: Repository,
}

impl GitEngine {
    // ── Construction ───────────────────────────────────────────────────

    /// Open an existing repository, discovering it from the given path.
    ///
    /// This works whether `path` points at the main worktree, a linked
    /// worktree, or any subdirectory inside either.
    pub fn open(path: &Path) -> Result<Self> {
        let repo = Repository::discover(path)
            .with_context(|| format!("failed to discover git repository from {}", path.display()))?;
        Ok(Self { repo })
    }

    /// Return the HEAD commit OID as a hex string.
    pub fn head_oid_string(&self) -> Result<String> {
        let head = self.repo.head()?.peel_to_commit()?.id();
        Ok(head.to_string())
    }

    // ── Worktree enumeration ─────────────────────────────────────────

    /// List all worktrees (the main one and any linked ones) with their
    /// branch, status counts, and last commit info.
    pub fn list_worktrees(&self) -> Result<Vec<WorktreeInfo>> {
        let mut infos: Vec<WorktreeInfo> = Vec::new();

        // 1. Main worktree — the one that owns .git/
        let main_path = self.main_worktree_path()?;
        match self.worktree_info_at(&main_path, true) {
            Ok(info) => infos.push(info),
            Err(e) => {
                log::warn!("failed to inspect main worktree at {}: {e}", main_path.display());
            }
        }

        // 2. Linked worktrees reported by libgit2
        if let Ok(worktree_names) = self.repo.worktrees() {
            for name in worktree_names.iter().flatten() {
                match self.linked_worktree_info(name) {
                    Ok(info) => infos.push(info),
                    Err(e) => {
                        log::warn!("failed to inspect linked worktree '{name}': {e}");
                    }
                }
            }
        }

        Ok(infos)
    }

    // ── Local branch listing ────────────────────────────────────

    /// Return a sorted list of all local branch names.
    pub fn list_local_branches(&self) -> Result<Vec<String>> {
        let branches = self.repo.branches(Some(git2::BranchType::Local))?;
        let mut names: Vec<String> = branches
            .filter_map(|b| {
                let (branch, _) = b.ok()?;
                branch.name().ok()?.map(String::from)
            })
            .collect();
        names.sort();
        Ok(names)
    }

    // ── Branch prefix helpers ────────────────────────────────────

    /// Strip common branch prefixes (feature/, fix/, etc.) to derive a
    /// short directory name.
    pub fn strip_branch_prefix(branch: &str) -> &str {
        for prefix in &["feature/", "fix/", "bugfix/", "hotfix/", "release/", "chore/"] {
            if let Some(rest) = branch.strip_prefix(prefix) {
                return rest;
            }
        }
        branch
    }

    /// Return the base directory for worktrees.
    ///
    /// Resolution order:
    /// 1. `CONDUCTOR_WORKTREE_DIR` environment variable
    /// 2. `override_dir` (from config `general.worktree_dir`)
    /// 3. Default: `<main-repo-parent>/<repo-name>-worktrees/`
    ///
    /// Creates the directory if it does not exist.
    pub fn worktrees_base_dir(&self, override_dir: Option<&Path>) -> Result<PathBuf> {
        let base = if let Ok(env_dir) = std::env::var("CONDUCTOR_WORKTREE_DIR") {
            PathBuf::from(env_dir)
        } else if let Some(dir) = override_dir {
            dir.to_path_buf()
        } else {
            let main_path = self.main_worktree_path()?;
            let repo_name = main_path
                .file_name()
                .ok_or_else(|| anyhow!("cannot determine repo name"))?
                .to_string_lossy();
            let parent = main_path
                .parent()
                .ok_or_else(|| anyhow!("cannot determine parent directory"))?;
            parent.join(format!("{repo_name}-worktrees"))
        };
        if !base.exists() {
            std::fs::create_dir_all(&base)
                .with_context(|| format!("failed to create worktrees base dir: {}", base.display()))?;
        }
        Ok(base)
    }

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
        let main_dir = self.main_worktree_path()?;
        let output = std::process::Command::new("git")
            .args([
                "worktree", "add",
                "-b", branch_name,
                &wt_path.display().to_string(),
                base_ref,
            ])
            .current_dir(&main_dir)
            .output()
            .context("failed to run `git worktree add`")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("git worktree add failed: {}", stderr.trim());
        }

        Ok(wt_path)
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
        let main_dir = self.main_worktree_path()?;
        let output = std::process::Command::new("git")
            .args([
                "worktree", "add",
                "--track", "-b", local_branch,
                &wt_path.display().to_string(),
                remote_branch,
            ])
            .current_dir(&main_dir)
            .output()
            .context("failed to run `git worktree add`")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("git worktree add failed: {}", stderr.trim());
        }

        Ok(wt_path)
    }

    // ── Enhanced deletion (wt rm -b -f) ─────────────────────────

    /// Delete a local branch by name. If `force` is true, uses -D (deletes
    /// even if not fully merged).
    pub fn delete_branch(&self, name: &str, force: bool) -> Result<()> {
        let mut branch = self.repo.find_branch(name, git2::BranchType::Local)
            .with_context(|| format!("branch '{name}' not found"))?;
        if force {
            // Force-delete: just delete the reference directly.
            let ref_name = format!("refs/heads/{name}");
            if let Ok(mut reference) = self.repo.find_reference(&ref_name) {
                reference.delete()
                    .with_context(|| format!("failed to force-delete branch '{name}'"))?;
            }
        } else {
            branch.delete()
                .with_context(|| format!("failed to delete branch '{name}' (not fully merged?)"))?;
        }
        Ok(())
    }

    /// Forcefully remove a worktree even if dirty.
    #[allow(dead_code)]
    pub fn remove_worktree_force(&self, worktree_path: &Path) -> Result<()> {
        let name = self.find_worktree_name_by_path(worktree_path)
            .with_context(|| format!("no worktree found for path {}", worktree_path.display()))?;
        let wt = self.repo.find_worktree(&name)
            .with_context(|| format!("worktree '{name}' not found"))?;

        let wt_path = wt.path().to_path_buf();

        // Prune with all flags to force removal.
        wt.prune(Some(
            git2::WorktreePruneOptions::new()
                .working_tree(true)
                .valid(true)
                .locked(true)
        )).with_context(|| format!("failed to force-prune worktree '{name}'"))?;

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
                if let Ok(wt) = self.repo.find_worktree(name) {
                    if wt.validate().is_err() {
                        stale.push(name.to_string());
                    }
                }
            }
        }
        Ok(stale)
    }

    /// Prune a single stale worktree entry.
    pub fn prune_stale_worktree(&self, name: &str) -> Result<()> {
        let wt = self.repo.find_worktree(name)
            .with_context(|| format!("worktree '{name}' not found"))?;

        wt.prune(Some(
            git2::WorktreePruneOptions::new()
                .working_tree(true)
        )).with_context(|| format!("failed to prune stale worktree '{name}'"))?;

        Ok(())
    }

    /// Force-prune a worktree entry by name, regardless of validity.
    /// Best-effort: silently ignores errors (entry may not exist).
    /// Used before creating a new worktree to clean up lingering entries.
    fn force_prune_worktree_entry(&self, name: &str) {
        if let Ok(wt) = self.repo.find_worktree(name) {
            let _ = wt.prune(Some(
                git2::WorktreePruneOptions::new()
                    .valid(true)
                    .working_tree(true)
            ));
        }
    }

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

    /// Grab a branch: move the source worktree to a temporary `__grab`
    /// branch, then checkout main to the original branch.
    ///
    /// Requires both worktrees to have no uncommitted tracked changes.
    pub fn grab_branch(
        &self,
        main_path: &Path,
        source_worktree_path: &Path,
        branch_name: &str,
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
        let source_repo = Repository::open(source_worktree_path)
            .with_context(|| format!("cannot open worktree at {}", source_worktree_path.display()))?;
        let head_commit = source_repo.head()?.peel_to_commit()?;
        source_repo.branch(&grab_branch_name, &head_commit, false)
            .with_context(|| format!("failed to create branch '{grab_branch_name}'"))?;
        source_repo.set_head(&format!("refs/heads/{grab_branch_name}"))
            .with_context(|| format!("failed to set HEAD to '{grab_branch_name}'"))?;
        source_repo.checkout_head(Some(git2::build::CheckoutBuilder::default().force()))
            .context("failed to checkout __grab branch")?;

        // Checkout main worktree to the original branch.
        let main_repo = Repository::open(main_path)
            .with_context(|| format!("cannot open main worktree at {}", main_path.display()))?;
        main_repo.set_head(&format!("refs/heads/{branch_name}"))
            .with_context(|| format!("failed to set main HEAD to '{branch_name}'"))?;
        main_repo.checkout_head(Some(git2::build::CheckoutBuilder::default().force()))
            .with_context(|| format!("failed to checkout '{branch_name}' in main worktree"))?;

        Ok(())
    }

    /// Ungrab: return main to main branch, restore source worktree to
    /// original branch, and delete the temporary `__grab` branch.
    ///
    /// Requires both worktrees to have no uncommitted tracked changes.
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
        let main_repo = Repository::open(main_path)
            .with_context(|| format!("cannot open main worktree at {}", main_path.display()))?;
        main_repo.set_head(&format!("refs/heads/{main_branch}"))
            .with_context(|| format!("failed to set main HEAD to '{main_branch}'"))?;
        main_repo.checkout_head(Some(git2::build::CheckoutBuilder::default().force()))
            .with_context(|| format!("failed to checkout '{main_branch}' in main worktree"))?;

        // Checkout source worktree back to original branch.
        let source_repo = Repository::open(source_worktree_path)
            .with_context(|| format!("cannot open worktree at {}", source_worktree_path.display()))?;
        source_repo.set_head(&format!("refs/heads/{branch_name}"))
            .with_context(|| format!("failed to set HEAD to '{branch_name}'"))?;
        source_repo.checkout_head(Some(git2::build::CheckoutBuilder::default().force()))
            .with_context(|| format!("failed to checkout '{branch_name}'"))?;

        // Delete the temporary __grab branch.
        let grab_branch_name = format!("{branch_name}__grab");
        let mut grab_branch = source_repo
            .find_branch(&grab_branch_name, git2::BranchType::Local)
            .with_context(|| format!("branch '{grab_branch_name}' not found"))?;
        grab_branch.delete()
            .with_context(|| format!("failed to delete branch '{grab_branch_name}'"))?;

        Ok(())
    }

    // ── PR URL ───────────────────────────────────────────────────

    /// Build a GitHub/GitLab pull-request URL for the given branch.
    ///
    /// Reads the `origin` remote URL, converts it to an HTTPS base, and
    /// appends the platform-specific path for creating a new pull request.
    /// Returns `None` if the remote URL cannot be parsed.
    pub fn pr_url_for_branch(&self, branch: &str) -> Option<String> {
        let remote = self.repo.find_remote("origin").ok()?;
        let raw_url = remote.url()?;
        let base = Self::remote_url_to_https_base(raw_url)?;

        // GitHub: /compare/<branch>  (shows existing PR or create form)
        // GitLab: /-/merge_requests/new?merge_request[source_branch]=<branch>
        if base.contains("gitlab") {
            Some(format!(
                "{base}/-/merge_requests/new?merge_request[source_branch]={branch}",
            ))
        } else {
            // Default to GitHub-style.
            Some(format!("{base}/pull/{branch}"))
        }
    }

    /// Convert a git remote URL to an HTTPS base URL (no trailing slash).
    ///
    /// Handles SSH (`git@host:owner/repo.git`) and HTTPS
    /// (`https://host/owner/repo.git`) formats.
    fn remote_url_to_https_base(url: &str) -> Option<String> {
        let url = url.trim();
        if url.starts_with("git@") || url.starts_with("ssh://") {
            // git@github.com:owner/repo.git  →  https://github.com/owner/repo
            // ssh://git@github.com/owner/repo.git
            let without_prefix = url
                .strip_prefix("ssh://")
                .unwrap_or(url)
                .strip_prefix("git@")
                .unwrap_or(url);
            // "github.com:owner/repo.git" or "github.com/owner/repo.git"
            let normalised = without_prefix.replace(':', "/");
            let trimmed = normalised.trim_end_matches(".git");
            Some(format!("https://{trimmed}"))
        } else if url.starts_with("https://") || url.starts_with("http://") {
            let trimmed = url.trim_end_matches(".git");
            Some(trimmed.to_string())
        } else {
            None
        }
    }

    // ── Fetch ────────────────────────────────────────────────────

    /// Run `git fetch --prune origin` by shelling out to the `git` CLI.
    ///
    /// libgit2's built-in credential handling doesn't support many common
    /// setups (macOS Keychain, `gh auth`, credential-manager-core, etc.),
    /// so we delegate to the real `git` binary which handles all of them.
    ///
    /// NOTE: This performs network I/O and may block for several seconds.
    /// Do NOT call from the UI thread — use a background thread instead.
    pub fn fetch_origin(&self) -> Result<()> {
        use std::process::{Command, Stdio};
        use std::time::Duration;

        let cwd = self.repo.workdir().unwrap_or(self.repo.path());
        log::debug!("fetch_origin: running `git fetch --prune origin` in {}", cwd.display());
        let mut child = Command::new("git")
            .args(["fetch", "--prune", "origin"])
            .current_dir(cwd)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to spawn `git fetch`")?;

        // Wait with a timeout so we never hang the background thread.
        let timeout = Duration::from_secs(30);
        let start = std::time::Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    // Process exited.
                    if !status.success() {
                        let stderr = child.stderr.take()
                            .map(|mut s| {
                                let mut buf = String::new();
                                std::io::Read::read_to_string(&mut s, &mut buf).ok();
                                buf
                            })
                            .unwrap_or_default();
                        log::warn!("fetch_origin stderr: {stderr}");
                        anyhow::bail!("git fetch failed (exit {}): {}", status, stderr.trim());
                    }
                    log::debug!("fetch_origin: success");
                    return Ok(());
                }
                Ok(None) => {
                    // Still running.
                    if start.elapsed() > timeout {
                        let _ = child.kill();
                        anyhow::bail!("git fetch timed out after {timeout:?}");
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => {
                    anyhow::bail!("failed to wait for git fetch: {e}");
                }
            }
        }
    }

    /// Remove a linked worktree by name.
    ///
    /// This prunes the worktree entry and optionally removes the directory.
    /// Cannot remove the main worktree.
    pub fn remove_worktree(&self, worktree_path: &Path) -> Result<()> {
        let name = self.find_worktree_name_by_path(worktree_path)
            .with_context(|| format!("no worktree found for path {}", worktree_path.display()))?;
        let wt = self.repo.find_worktree(&name)
            .with_context(|| format!("worktree '{name}' not found"))?;

        let wt_path = wt.path().to_path_buf();

        // Validate it first
        if wt.validate().is_ok() {
            // Worktree is valid and exists — prune it
            wt.prune(Some(
                git2::WorktreePruneOptions::new()
                    .working_tree(true)
                    .valid(true)
            )).with_context(|| format!("failed to prune worktree '{name}'"))?;
        } else {
            // Worktree is already invalid (e.g. directory deleted) — just prune
            wt.prune(Some(
                git2::WorktreePruneOptions::new()
                    .working_tree(true)
            )).with_context(|| format!("failed to prune worktree '{name}'"))?;
        }

        // Remove the directory if it still exists
        if wt_path.exists() {
            std::fs::remove_dir_all(&wt_path)
                .with_context(|| format!("failed to remove worktree directory {}", wt_path.display()))?;
        }

        Ok(())
    }

    // ── Pull (fetch + fast-forward) ────────────────────────────────────

    /// Fetch from origin and fast-forward the branch in the given worktree.
    ///
    /// Returns a human-readable status message describing the outcome.
    /// Only fast-forward merges are performed; non-FF situations are reported
    /// so the user can resolve them manually.
    ///
    /// NOTE: calls `fetch_origin()` internally, so this performs network I/O.
    /// Must be called from a background thread.
    pub fn pull_worktree(&self, worktree_path: &Path) -> Result<String> {
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
        let upstream_name = upstream
            .name()?
            .unwrap_or("unknown")
            .to_string();

        // Fetch from origin (updates all remote refs).
        self.fetch_origin()?;

        // Re-open the repo to pick up the updated remote refs.
        let wt_repo = Repository::open(worktree_path)
            .with_context(|| format!("cannot re-open worktree at {}", worktree_path.display()))?;

        // Resolve upstream OID after fetch.
        let upstream_ref = wt_repo
            .find_reference(&format!("refs/remotes/{upstream_name}"))
            .with_context(|| format!("upstream ref 'refs/remotes/{upstream_name}' not found after fetch"))?;
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
        main_repo.reference(
            "refs/original/ORIG_HEAD",
            head_commit.id(),
            true,
            "conductor: save ORIG_HEAD before merge",
        ).ok(); // best-effort

        // Find the branch to merge
        let branch_ref = main_repo.find_branch(branch_name, git2::BranchType::Local)
            .with_context(|| format!("branch '{branch_name}' not found"))?;
        let branch_commit_oid = branch_ref.get().peel_to_commit()?.id();
        let branch_annotated = main_repo.find_annotated_commit(branch_commit_oid)
            .context("failed to find annotated commit for branch")?;

        // Perform merge analysis
        let (analysis, _preference) = main_repo.merge_analysis(&[&branch_annotated])?;

        if analysis.is_up_to_date() {
            return Ok(format!("{main_branch} is already up-to-date with {branch_name}."));
        }

        if analysis.is_fast_forward() {
            // Fast-forward: just move the main branch ref
            let mut main_ref = main_repo.find_reference(&format!("refs/heads/{main_branch}"))?;
            main_ref.set_target(
                branch_commit_oid,
                &format!("conductor: fast-forward merge {branch_name} into {main_branch}"),
            )?;
            // Update HEAD / working directory
            main_repo.checkout_head(Some(
                git2::build::CheckoutBuilder::new().force()
            ))?;
            return Ok(format!("Fast-forward merged {branch_name} into {main_branch}."));
        }

        if analysis.is_normal() {
            // Normal merge — this is more complex and can conflict.
            // For safety, we'll report that a non-fast-forward merge is needed
            // and recommend the user do it manually.
            return Ok(format!(
                "Cannot fast-forward. Manual merge needed: cd {} && git merge {}",
                main_path.display(), branch_name
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
        if let Ok(head) = main_repo.head() {
            if let Ok(commit) = head.peel_to_commit() {
                main_repo.reference(
                    "refs/original/ORIG_HEAD",
                    commit.id(),
                    true,
                    "conductor: save ORIG_HEAD before reset",
                ).ok();
            }
        }

        // Find origin/<main_branch>
        let remote_ref_name = format!("refs/remotes/origin/{main_branch}");
        let remote_ref = main_repo.find_reference(&remote_ref_name)
            .with_context(|| format!("remote ref '{remote_ref_name}' not found. Have you fetched?"))?;
        let remote_commit = remote_ref.peel_to_commit()
            .context("remote ref does not point to a commit")?;

        // Reset to the remote commit
        let obj = remote_commit.as_object();
        main_repo.reset(obj, git2::ResetType::Hard, None)
            .context("failed to hard reset")?;

        Ok(format!("Reset {main_branch} to origin/{main_branch} (commit {}).",
            &remote_commit.id().to_string()[..8]))
    }

    // ── Cherry-pick helpers ───────────────────────────────────────────

    /// List up to `limit` commits from the given branch, newest first.
    pub fn list_branch_commits(&self, branch_name: &str, limit: usize) -> Result<Vec<CommitInfo>> {
        let branch = self.repo.find_branch(branch_name, git2::BranchType::Local)
            .with_context(|| format!("branch '{branch_name}' not found"))?;
        let commit = branch.get().peel_to_commit()
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

            let message = c.message()
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
    pub fn cherry_pick_to_worktree(&self, worktree_path: &Path, commit_oid_str: &str) -> Result<String> {
        let repo = Repository::open(worktree_path)
            .with_context(|| format!("cannot open worktree repo at {}", worktree_path.display()))?;

        let oid = git2::Oid::from_str(commit_oid_str)
            .with_context(|| format!("invalid OID: {commit_oid_str}"))?;
        let commit = repo.find_commit(oid)
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
        let sig = repo.signature()
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
        let msg_first_line = commit.message()
            .unwrap_or("")
            .lines()
            .next()
            .unwrap_or("");
        Ok(format!("Cherry-picked {short} \"{msg_first_line}\" successfully."))
    }

    // ── Internal helpers ─────────────────────────────────────────────

    /// Resolve a ref string (branch name, remote ref, or tag) to a `Commit`.
    #[allow(dead_code)]
    fn resolve_ref_to_commit(&self, refspec: &str) -> Result<git2::Commit<'_>> {
        // Try as a direct reference first (e.g. "refs/remotes/origin/main").
        if let Ok(reference) = self.repo.find_reference(&format!("refs/remotes/{refspec}")) {
            return reference.peel_to_commit()
                .with_context(|| format!("ref '{refspec}' does not point to a commit"));
        }
        if let Ok(reference) = self.repo.find_reference(&format!("refs/heads/{refspec}")) {
            return reference.peel_to_commit()
                .with_context(|| format!("ref '{refspec}' does not point to a commit"));
        }
        // Try revparse as a fallback.
        let obj = self.repo.revparse_single(refspec)
            .with_context(|| format!("cannot resolve '{refspec}'"))?;
        obj.peel_to_commit()
            .with_context(|| format!("'{refspec}' does not point to a commit"))
    }

    /// Determine the absolute path to the main (primary) worktree.
    ///
    /// When opened from a linked worktree, `repo.workdir()` returns *that*
    /// worktree's path, not the main one.  We detect this by inspecting the
    /// git dir structure: linked worktrees have their git dir at
    /// `<main>/.git/worktrees/<name>/`.
    pub fn main_worktree_path(&self) -> Result<PathBuf> {
        let git_dir = self.repo.path(); // linked: <main>/.git/worktrees/<name>/
                                         // main:   <main>/.git/

        // If git_dir is inside .git/worktrees/, walk up to the main repo root.
        if let Some(worktrees_dir) = git_dir.parent() {
            if worktrees_dir.file_name() == Some("worktrees".as_ref()) {
                if let Some(dot_git) = worktrees_dir.parent() {
                    if let Some(main_repo) = dot_git.parent() {
                        return Ok(main_repo.to_path_buf());
                    }
                }
            }
        }

        // Normal (non-bare) repository.
        if let Some(workdir) = self.repo.workdir() {
            return Ok(workdir.to_path_buf());
        }

        // Bare repo — the "main worktree" is the git dir's parent.
        git_dir
            .parent()
            .map(|p| p.to_path_buf())
            .ok_or_else(|| anyhow!("cannot determine main worktree path"))
    }

    /// Find the libgit2 worktree name that corresponds to the given path.
    ///
    /// Worktree names may differ from branch names (e.g. `feature/foo`
    /// creates a worktree named `foo`), so we iterate all registered
    /// worktrees and match by path.
    fn find_worktree_name_by_path(&self, target: &Path) -> Option<String> {
        let names = self.repo.worktrees().ok()?;
        for name in names.iter().flatten() {
            if let Ok(wt) = self.repo.find_worktree(name) {
                if wt.path() == target {
                    return Some(name.to_string());
                }
            }
        }
        None
    }

    /// Build `WorktreeInfo` for a linked worktree identified by its
    /// libgit2 name.
    fn linked_worktree_info(&self, name: &str) -> Result<WorktreeInfo> {
        let wt = self.repo.find_worktree(name)?;
        let wt_path = wt
            .path()
            .to_path_buf();

        self.worktree_info_at(&wt_path, false)
    }

    /// Build `WorktreeInfo` by opening the repository at `path`.
    fn worktree_info_at(&self, path: &Path, is_main: bool) -> Result<WorktreeInfo> {
        let repo = Repository::open(path)
            .with_context(|| format!("cannot open repo at {}", path.display()))?;

        let branch = Self::current_branch_name(&repo);
        let (added, modified, deleted) = Self::status_counts(&repo).unwrap_or((0, 0, 0));
        let is_clean = added == 0 && modified == 0 && deleted == 0;
        let (ahead, behind) = Self::ahead_behind_upstream(&repo);

        Ok(WorktreeInfo {
            path: path.to_path_buf(),
            branch,
            is_main,
            added,
            modified,
            deleted,
            is_clean,
            ahead,
            behind,
        })
    }

    /// Compute ahead/behind counts relative to the upstream tracking branch.
    /// Returns `(None, None)` if there is no upstream or on error.
    fn ahead_behind_upstream(repo: &Repository) -> (Option<usize>, Option<usize>) {
        let head = match repo.head() {
            Ok(h) if h.is_branch() => h,
            _ => return (None, None),
        };
        let local_oid = match head.target() {
            Some(oid) => oid,
            None => return (None, None),
        };
        let branch_name = match head.shorthand() {
            Some(name) => name.to_string(),
            None => return (None, None),
        };
        let branch = match repo.find_branch(&branch_name, git2::BranchType::Local) {
            Ok(b) => b,
            Err(_) => return (None, None),
        };
        let upstream = match branch.upstream() {
            Ok(u) => u,
            Err(_) => return (None, None),
        };
        let upstream_oid = match upstream.get().target() {
            Some(oid) => oid,
            None => return (None, None),
        };
        match repo.graph_ahead_behind(local_oid, upstream_oid) {
            Ok((ahead, behind)) => (Some(ahead), Some(behind)),
            Err(_) => (None, None),
        }
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

    /// Get the current branch name, or `"HEAD (detached)"` if detached.
    fn current_branch_name(repo: &Repository) -> String {
        if let Ok(head) = repo.head() {
            if head.is_branch() {
                if let Some(name) = head.shorthand() {
                    return name.to_string();
                }
            }
        }
        "HEAD (detached)".to_string()
    }

    /// Compute `(added, modified, deleted)` status counts for a repository.
    fn status_counts(repo: &Repository) -> Result<(usize, usize, usize)> {
        let mut opts = StatusOptions::new();
        opts.show(StatusShow::IndexAndWorkdir)
            .include_untracked(true)
            .renames_head_to_index(true);

        let statuses = repo.statuses(Some(&mut opts))?;

        let mut added: usize = 0;
        let mut modified: usize = 0;
        let mut deleted: usize = 0;

        for entry in statuses.iter() {
            let s = entry.status();
            // Index changes
            if s.intersects(git2::Status::INDEX_NEW) {
                added += 1;
            } else if s.intersects(git2::Status::INDEX_MODIFIED | git2::Status::INDEX_RENAMED | git2::Status::INDEX_TYPECHANGE) {
                modified += 1;
            } else if s.intersects(git2::Status::INDEX_DELETED) {
                deleted += 1;
            }
            // Working-directory changes (only count if not already counted
            // from the index side above).  We use `else if` chains so each
            // file is counted at most once.
            else if s.intersects(git2::Status::WT_NEW) {
                added += 1;
            } else if s.intersects(git2::Status::WT_MODIFIED | git2::Status::WT_RENAMED | git2::Status::WT_TYPECHANGE) {
                modified += 1;
            } else if s.intersects(git2::Status::WT_DELETED) {
                deleted += 1;
            }
        }

        Ok((added, modified, deleted))
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    /// Smoke test: open the repository that contains this very source file.
    #[test]
    fn open_this_repo() {
        let manifest = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
        let _engine = GitEngine::open(Path::new(&manifest)).expect("should open repo");
    }

    /// Smoke test: list worktrees (should include at least the main one).
    #[test]
    fn list_worktrees_includes_main() {
        let manifest = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
        let engine = GitEngine::open(Path::new(&manifest)).expect("should open repo");
        let worktrees = engine.list_worktrees().expect("list_worktrees() failed");
        assert!(!worktrees.is_empty(), "expected at least one worktree");
        assert!(
            worktrees.iter().any(|w| w.is_main),
            "expected one worktree to be marked as main"
        );
    }

    /// Verify that `main_worktree_path()` returns the correct path even when
    /// opened from a linked worktree.
    #[test]
    fn main_worktree_path_from_linked_worktree() {
        use std::fs;

        // Create a temporary bare-bones git repo and a linked worktree.
        let tmp = tempfile::tempdir().expect("create temp dir");
        let main_repo_path = tmp.path().join("main-repo");
        fs::create_dir_all(&main_repo_path).unwrap();

        // Init the main repo and create an initial commit.
        let repo = Repository::init(&main_repo_path).expect("init repo");
        {
            let mut index = repo.index().unwrap();
            let oid = index.write_tree().unwrap();
            let tree = repo.find_tree(oid).unwrap();
            let sig = git2::Signature::now("Test", "test@test.com").unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "initial commit", &tree, &[]).unwrap();
        }

        // Create a linked worktree.
        let wt_path = tmp.path().join("linked-wt");
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("test-branch", &head, false).unwrap();
        let branch_ref = repo.find_reference("refs/heads/test-branch").unwrap();
        repo.worktree(
            "test-branch",
            &wt_path,
            Some(git2::WorktreeAddOptions::new().reference(Some(&branch_ref))),
        ).expect("create linked worktree");

        // Open from the linked worktree and verify main_worktree_path().
        let engine = GitEngine::open(&wt_path).expect("open from linked worktree");
        let main_path = engine.main_worktree_path().expect("main_worktree_path()");

        // Canonicalize both paths for comparison (temp dirs may use symlinks).
        let expected = main_repo_path.canonicalize().unwrap();
        let actual = main_path.canonicalize().unwrap();
        assert_eq!(actual, expected, "main_worktree_path() should return main repo, not linked worktree");
    }

    /// Verify that `main_worktree_path()` works from the main repo too.
    #[test]
    fn main_worktree_path_from_main_repo() {
        let manifest = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
        let engine = GitEngine::open(Path::new(&manifest)).expect("should open repo");
        let main_path = engine.main_worktree_path().expect("main_worktree_path()");
        // The main worktree path should exist and contain a .git directory.
        assert!(main_path.exists(), "main worktree path should exist");
        assert!(main_path.join(".git").exists(), "main worktree should contain .git");
    }

    #[test]
    fn remote_url_to_https_base_ssh() {
        assert_eq!(
            GitEngine::remote_url_to_https_base("git@github.com:owner/repo.git"),
            Some("https://github.com/owner/repo".to_string()),
        );
    }

    #[test]
    fn remote_url_to_https_base_https() {
        assert_eq!(
            GitEngine::remote_url_to_https_base("https://github.com/owner/repo.git"),
            Some("https://github.com/owner/repo".to_string()),
        );
    }

    #[test]
    fn remote_url_to_https_base_no_suffix() {
        assert_eq!(
            GitEngine::remote_url_to_https_base("https://github.com/owner/repo"),
            Some("https://github.com/owner/repo".to_string()),
        );
    }

    #[test]
    fn remote_url_to_https_base_ssh_prefix() {
        assert_eq!(
            GitEngine::remote_url_to_https_base("ssh://git@github.com/owner/repo.git"),
            Some("https://github.com/owner/repo".to_string()),
        );
    }
}
