//! Worktree/branch enumeration and per-worktree status snapshotting.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use git2::{Repository, StatusOptions, StatusShow};

use super::{GitEngine, WorktreeInfo};

impl GitEngine {
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
                log::warn!(
                    "failed to inspect main worktree at {}: {e}",
                    main_path.display()
                );
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
        for prefix in &[
            "feature/", "fix/", "bugfix/", "hotfix/", "release/", "chore/",
        ] {
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
            std::fs::create_dir_all(&base).with_context(|| {
                format!("failed to create worktrees base dir: {}", base.display())
            })?;
        }
        Ok(base)
    }

    // ── Internal: per-worktree status snapshot ────────────────────────

    /// Build `WorktreeInfo` for a linked worktree identified by its
    /// libgit2 name.
    pub(super) fn linked_worktree_info(&self, name: &str) -> Result<WorktreeInfo> {
        let wt = self.repo.find_worktree(name)?;
        let wt_path = wt.path().to_path_buf();

        self.worktree_info_at(&wt_path, false)
    }

    /// Build `WorktreeInfo` by opening the repository at `path`.
    pub(super) fn worktree_info_at(&self, path: &Path, is_main: bool) -> Result<WorktreeInfo> {
        let repo = Repository::open(path)
            .with_context(|| format!("cannot open repo at {}", path.display()))?;

        let branch = Self::current_branch_name(&repo);
        let (added, modified, deleted) = Self::status_counts(&repo).unwrap_or((0, 0, 0));
        let is_clean = added == 0 && modified == 0 && deleted == 0;
        let (ahead, behind) = Self::ahead_behind_upstream(&repo);
        let head_oid = repo
            .head()
            .ok()
            .and_then(|h| h.target())
            .map(|oid| oid.to_string());

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
            head_oid,
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

    /// Get the current branch name, or `"HEAD (detached)"` if detached.
    fn current_branch_name(repo: &Repository) -> String {
        if let Ok(head) = repo.head()
            && head.is_branch()
            && let Some(name) = head.shorthand()
        {
            return name.to_string();
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
            } else if s.intersects(
                git2::Status::INDEX_MODIFIED
                    | git2::Status::INDEX_RENAMED
                    | git2::Status::INDEX_TYPECHANGE,
            ) {
                modified += 1;
            } else if s.intersects(git2::Status::INDEX_DELETED) {
                deleted += 1;
            }
            // Working-directory changes (only count if not already counted
            // from the index side above).  We use `else if` chains so each
            // file is counted at most once.
            else if s.intersects(git2::Status::WT_NEW) {
                added += 1;
            } else if s.intersects(
                git2::Status::WT_MODIFIED | git2::Status::WT_RENAMED | git2::Status::WT_TYPECHANGE,
            ) {
                modified += 1;
            } else if s.intersects(git2::Status::WT_DELETED) {
                deleted += 1;
            }
        }

        Ok((added, modified, deleted))
    }

    // ── Main worktree path resolution ──────────────────────────────────

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
        if let Some(worktrees_dir) = git_dir.parent()
            && worktrees_dir.file_name() == Some("worktrees".as_ref())
            && let Some(dot_git) = worktrees_dir.parent()
            && let Some(main_repo) = dot_git.parent()
        {
            return Ok(main_repo.to_path_buf());
        }

        // Normal (non-bare) repository.
        // `repo.workdir()` may return a path with a trailing slash (libgit2
        // convention).  Normalize via `components().collect()` so the path
        // is consistent with linked-worktree paths and shell `$PWD` values.
        if let Some(workdir) = self.repo.workdir() {
            return Ok(workdir.components().collect());
        }

        // Bare repo — the "main worktree" is the git dir's parent.
        git_dir
            .parent()
            .map(|p| p.to_path_buf())
            .ok_or_else(|| anyhow!("cannot determine main worktree path"))
    }
}
