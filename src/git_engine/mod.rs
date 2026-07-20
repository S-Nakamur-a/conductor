//! Git operations powered by libgit2.
//!
//! Provides a high-level interface over `git2` for repository inspection:
//! worktree listing, status counts, commit info, diff generation, and more.
//!
//! The functionality is split by responsibility across sibling submodules,
//! all implementing methods on the shared [`GitEngine`] handle:
//!
//! - [`worktree_ops`]: worktree/branch enumeration and status snapshotting
//! - [`worktree_create`]: worktree creation (from a base ref, a remote branch,
//!   or an already-fetched branch) and base-ref freshness
//! - [`worktree_delete`]: branch deletion and worktree removal/pruning
//! - [`grab`]: the `wt grab`/`wt ungrab` branch-swap workflow
//! - [`branch_lineage`]: parent/derived branch detection and PR URL building
//! - [`fetch`]: shelling out to `git fetch`
//! - [`merge`]: pull, merge-into-main, and hard-reset-to-origin
//! - [`cherry_pick`]: listing commits and cherry-picking them into a worktree
//! - [`recently_modified`]: recently touched file paths for a worktree

mod branch_lineage;
mod cherry_pick;
mod fetch;
mod grab;
mod merge;
mod recently_modified;
#[cfg(test)]
mod tests;
mod worktree_create;
mod worktree_delete;
mod worktree_ops;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use git2::Repository;

pub use recently_modified::recently_modified_files;

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
    /// HEAD commit OID (hex). `None` on an unborn branch. Captured while the
    /// repo is already open so callers don't need a second `Repository::open`
    /// per worktree just to detect new commits.
    pub head_oid: Option<String>,
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

/// Branch lineage and PR information for the detail panel.
#[derive(Debug, Clone, Default)]
pub struct BranchDetails {
    /// The base (initial) branch this branch was created from.
    pub initial_branch: Option<String>,
    /// Branches that were forked from this branch.
    pub derived_branches: Vec<String>,
    /// GitHub PR URL for this branch (fetched via `gh`).
    pub pr_url: Option<String>,
    /// Whether a PR URL lookup is currently in progress.
    pub pr_loading: bool,
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
        let repo = Repository::discover(path).with_context(|| {
            format!("failed to discover git repository from {}", path.display())
        })?;
        Ok(Self { repo })
    }
}
